"""Restaurant demo agent using the A2UI 1.0 Basic Catalog and HTTP/SSE."""

import argparse
import asyncio
from collections.abc import Mapping
import json
import os
import re
import sys
import time
from datetime import date, timedelta
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.error import URLError
from urllib.parse import urlsplit, urlunsplit
from urllib.request import Request, urlopen

from a2ui.core.basic_catalog import BasicCatalog
from a2ui.core.schema import ProtocolVersion
from a2ui.processor import A2uiGenerator, CatalogConfig
from a2ui.rpc import A2uiRendererActionMessage, RendererToAgentMessageProcessor
from a2ui.utils import read_renderer_capabilities, validate_payload


VERSION = ProtocolVersion.V1_0
BASIC_CATALOG = BasicCatalog(VERSION)
GENERATOR = A2uiGenerator([CatalogConfig(BASIC_CATALOG)])
SURFACE = "restaurants"
RESTAURANTS = {
    "olive": {"id": "olive", "name": "Olive & Oak", "cuisine": "Italian",
              "description": "Handmade pasta · Open 18:00–23:00"},
    "forno": {"id": "forno", "name": "Forno House", "cuisine": "Italian",
              "description": "Wood-fired pizza · Open 17:00–23:00"},
    "juniper": {"id": "juniper", "name": "Juniper Kitchen", "cuisine": "Vegetarian",
                "description": "Seasonal plates · Open 12:00–21:00"},
}


def component(component_id, kind, **properties):
    return {"id": component_id, "component": kind, **properties}


def binding(path):
    return {"@path": path}


def text(component_id, content):
    return component(component_id, "Text", text=content)


def button(component_id, label, action, **context):
    label_id = f"{component_id}_label"
    return (
        component(component_id, "Button", child=label_id, variant="primary",
                  action={"event": {"name": action, "context": context}}),
        text(label_id, label),
    )


def wire(kind, **properties):
    return {"version": VERSION.value, kind: {"surfaceId": SURFACE, **properties}}


def show_surface(held, components, model):
    if SURFACE not in held:
        yield wire("createSurface", catalogId=BASIC_CATALOG.catalog_id, sendDataModel=True)
    yield wire("updateComponents", components=components)
    yield wire("updateDataModel", value=model)


def restaurant_list(held, cuisine):
    restaurants = [item for item in RESTAURANTS.values()
                   if cuisine is None or item["cuisine"] == cuisine]
    heading = f"{cuisine} restaurants" if cuisine else "Find a restaurant"
    return show_surface(held, [
        component("root", "Column", children=["heading", "restaurants"]),
        text("heading", f"# {heading}"),
        component("restaurants", "List", children={"componentId": "card", "path": "/restaurants"}),
        component("card", "Card", child="row"),
        component("row", "Row", align="center", justify="spaceBetween",
                  children=["info", "select"]),
        component("info", "Column", children=["name", "description"], weight=1),
        text("name", binding("name")),
        text("description", binding("description")),
        *button("select", "Reserve", "selectRestaurant", restaurantId=binding("id")),
    ], {"restaurants": restaurants})


def booking_form(held, restaurant, guests):
    values = {"restaurantId": restaurant["id"], "restaurant": f"# {restaurant['name']}",
              "guests": guests, "date": (date.today() + timedelta(days=1)).isoformat(),
              "seating": ["inside"]}
    return show_surface(held, [
        component("root", "Column", children=["heading", "description", "card"]),
        text("heading", binding("/booking/restaurant")),
        text("description", "Choose a date, party size, and seating for your demo reservation."),
        component("card", "Card", child="form"),
        component("form", "Column", children=["date", "guests", "seating", "confirm"]),
        component("date", "DateTimeInput", label="Date", enableDate=True, value=binding("/booking/date")),
        component("guests", "Slider", label="Guests", min=1, max=8, value=binding("/booking/guests")),
        component("seating", "ChoicePicker", label="Seating", variant="mutuallyExclusive",
                  displayStyle="chips", value=binding("/booking/seating"),
                  options=[{"label": "Inside", "value": "inside"},
                           {"label": "Terrace", "value": "terrace"}]),
        *button("confirm", "Confirm demo reservation", "reserveTable",
                **{key: binding(f"/booking/{key}") for key in ("restaurantId", "guests", "date", "seating")}),
    ], {"booking": values})


def confirmation(held, booking):
    return show_surface(held, [
        component("root", "Card", child="confirmation"),
        component("confirmation", "Column", children=["heading", "details", "next"]),
        text("heading", "# Demo reservation confirmed"),
        text("details", binding("/confirmation")),
        *button("next", "Find another restaurant", "searchAgain"),
    ], {"confirmation": booking})


def chat_completions_url(url):
    endpoint = urlsplit(url)
    if endpoint.scheme not in ("http", "https") or not endpoint.hostname:
        raise ValueError("--llm-url must be an HTTP(S) URL.")
    if endpoint.fragment:
        raise ValueError("--llm-url cannot contain a fragment.")
    path = endpoint.path.rstrip("/")
    if not path:
        path = "/v1/chat/completions"
    elif path.endswith("/v1"):
        path += "/chat/completions"
    return urlunsplit(endpoint._replace(path=path))


def discover_model(chat_url):
    endpoint = urlsplit(chat_url)
    if not endpoint.path.endswith("/chat/completions"):
        raise ValueError("Set --llm-model when using a custom chat-completions path.")
    base = endpoint.path.removesuffix("/chat/completions")
    models_url = urlunsplit(endpoint._replace(path=f"{base}/models"))
    try:
        with urlopen(models_url, timeout=5) as response:
            models = json.load(response)["data"]
        model_ids = list(dict.fromkeys(model["id"] for model in models))
        if any(not isinstance(model_id, str) or not model_id.strip() for model_id in model_ids):
            raise ValueError("invalid model ID")
    except (URLError, OSError, KeyError, TypeError, ValueError) as error:
        raise ValueError(f"Could not list local models at {models_url}: {error}") from error
    if not model_ids:
        raise ValueError("The local server reports no models.")
    if len(model_ids) != 1:
        raise ValueError("The local server reports multiple models. Set --llm-model to one model ID.")
    return model_ids[0]


def prompt_intent(prompt, llm_url, llm_model):
    if llm_url:
        body = {
            "model": llm_model or discover_model(llm_url),
            "messages": [
                {"role": "system", "content": (
                    'Interpret a restaurant request. Reply with a single JSON object: '
                    '{"intent":"search"|"book"|"hours","cuisine":"Italian"|"Vegetarian"|null,'
                    '"guests":1..8}. Use 2 guests by default. Use "search" unless the user '
                    'specifically wants to book/reserve or asks about hours.'
                )},
                {"role": "user", "content": prompt},
            ],
            "temperature": 0,
            "max_tokens": 256,
        }
        request = Request(llm_url, json.dumps(body).encode(), {"Content-Type": "application/json"})
        try:
            with urlopen(request, timeout=90) as response:
                reply = json.load(response)["choices"][0]["message"]["content"].strip()
            if reply.startswith("\x60\x60\x60"):
                reply = "\n".join(reply.splitlines()[1:-1])
            result = json.loads(reply)
            intent = result["intent"]
            cuisine = result["cuisine"]
            guests = result["guests"]
            if (intent not in ("search", "book", "hours") or cuisine not in (None, "Italian", "Vegetarian")
                    or type(guests) is not int or not 1 <= guests <= 8):
                raise ValueError("invalid restaurant intent")
            return intent, cuisine, guests
        except (URLError, OSError, KeyError, IndexError, ValueError, AttributeError, TypeError) as error:
            raise ValueError("The local model could not return a valid restaurant intent. " + str(error)) from error

    words = set(re.findall(r"[a-z]+", prompt.casefold()))
    cuisine = next((name for name in ("Italian", "Vegetarian") if name.lower() in words), None)
    match = re.search(r"\b([1-8])\b", prompt)
    intent = "hours" if "hours" in words else "book" if words & {"book", "reserve"} else "search"
    return intent, cuisine, int(match.group(1)) if match else 2


def action_turn(metadata, held, message: A2uiRendererActionMessage):
    action = message.action
    if action.surface_id != SURFACE or SURFACE not in held:
        raise ValueError("The action does not refer to the current restaurant surface.")
    context = action.context
    name = action.name
    if name == "searchAgain":
        return "Here are the available restaurants.", restaurant_list(held, None)
    restaurant_id = context.get("restaurantId")
    restaurant = RESTAURANTS.get(restaurant_id) if isinstance(restaurant_id, str) else None
    if restaurant is None:
        raise ValueError("Select an available restaurant.")
    if name == "selectRestaurant":
        return "Your reservation form is ready.", booking_form(held, restaurant, 2)
    if name != "reserveTable":
        raise ValueError("Unknown restaurant action.")

    data_model = metadata.get("a2uiRendererDataModel", {})
    surfaces = data_model.get("surfaces", {}) if isinstance(data_model, dict) else {}
    model = surfaces.get(SURFACE, {}) if isinstance(surfaces, dict) else {}
    booking = model.get("booking", {}) if isinstance(model, dict) else {}
    if not isinstance(booking, dict) or any(
        booking.get(key) != context.get(key) for key in ("restaurantId", "guests", "date", "seating")
    ):
        raise ValueError("The submitted action does not match the form data.")
    guests = context.get("guests")
    seating = context.get("seating")
    if type(guests) not in (int, float) or not 1 <= guests <= 8 or int(guests) != guests:
        raise ValueError("Choose a party size from 1 to 8.")
    if seating not in (["inside"], ["terrace"]):
        raise ValueError("Choose a seating preference.")
    try:
        day = date.fromisoformat(context.get("date", ""))
    except (ValueError, TypeError) as error:
        raise ValueError("Choose a valid date.") from error
    summary = f"{restaurant['name']} · {int(guests)} guests · {day.isoformat()} · {seating[0].title()}"
    return "The agent received your edited form values.", confirmation(held, summary)


def prompt_turn(held, prompt, llm_url, llm_model):
    if not isinstance(prompt, str) or not prompt.strip() or len(prompt) > 2_000:
        raise ValueError("Enter a restaurant request of up to 2,000 characters.")
    intent, cuisine, guests = prompt_intent(prompt, llm_url, llm_model)
    if intent == "hours":
        return "Opening hours are shown on each card.", restaurant_list(held, cuisine)
    if intent == "book":
        restaurant = next(item for item in RESTAURANTS.values()
                          if cuisine is None or item["cuisine"] == cuisine)
        return "Your reservation form is ready.", booking_form(held, restaurant, guests)
    return "Pick a restaurant to open a reservation form.", restaurant_list(held, cuisine)


async def turn(request, llm_url=None, llm_model=None):
    held = request.get("surfaces", [])
    if not isinstance(held, list) or any(not isinstance(item, str) for item in held):
        raise ValueError("Surface IDs must be a list of strings.")

    metadata = request.get("metadata")
    if not isinstance(metadata, Mapping):
        raise ValueError("Expected A2UI renderer metadata.")
    capabilities = read_renderer_capabilities(metadata, VERSION)
    if capabilities is None:
        raise ValueError("This demo needs A2UI 1.0 renderer capabilities.")
    catalogs = GENERATOR.create_processor(capabilities).active_catalogs

    envelope = request.get("message")
    if envelope is None:
        response, messages = prompt_turn(held, request.get("prompt"), llm_url, llm_model)
    else:
        if not isinstance(envelope, dict):
            raise ValueError("An action must be an A2UI object.")
        outcome = None

        def on_action(message: A2uiRendererActionMessage):
            nonlocal outcome
            outcome = action_turn(metadata, held, message)

        await RendererToAgentMessageProcessor(catalogs, on_action=on_action).process_message(envelope)
        if outcome is None:
            raise ValueError("This demo accepts renderer action messages.")
        response, messages = outcome

    # Validate the whole turn before streaming any updates. The client reports
    # which surfaces it holds, so there is no agent-side state to carry across turns.
    messages = list(messages)
    existing = {SURFACE: BASIC_CATALOG.catalog_id} if SURFACE in held else {}
    validate_payload(catalogs, messages, surface_catalog_ids=existing)
    return response, messages


def encode_event(payload, name=None):
    event = f"event: {name}\n" if name else ""
    return f"{event}data: {json.dumps(payload, ensure_ascii=False)}\n\n".encode("utf-8")


def stream_turn(request, output, llm_url=None, llm_model=None):
    try:
        response, messages = asyncio.run(turn(request, llm_url, llm_model))
    except ValueError as error:
        output.write(encode_event(str(error), "error"))
        output.flush()
        return
    output.write(encode_event(response, "text"))
    output.flush()
    for message in messages:
        output.write(encode_event(message))
        output.flush()
        time.sleep(0.12)


def decode_body(body):
    if not 0 < len(body) <= 256 * 1024:
        raise ValueError("Expected a JSON request of at most 256 KiB.")
    request = json.loads(body)
    if not isinstance(request, dict):
        raise ValueError("Expected a JSON object.")
    return request


class AgentHandler(BaseHTTPRequestHandler):
    def do_POST(self):
        if self.path != "/api/agent":
            self.send_error(404)
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            if not 0 < length <= 256 * 1024:
                raise ValueError
            request = decode_body(self.rfile.read(length))
        except (ValueError, UnicodeError):
            self.send_error(400, "Expected a JSON request of at most 256 KiB.")
            return

        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream; charset=utf-8")
        self.send_header("Cache-Control", "no-cache")
        self.end_headers()
        try:
            stream_turn(request, self.wfile, self.server.llm_url, self.server.llm_model)
        except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
            return


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=8765)
    parser.add_argument("--stdio", action="store_true", help="Handle one request on stdin.")
    parser.add_argument("--llm-url", default=os.getenv("A2UI_LLM_URL"),
                        help="Local model server base URL or complete chat-completions URL.")
    parser.add_argument("--llm-model", default=os.getenv("A2UI_LLM_MODEL"),
                        help="Model ID. The agent can discover it when the server lists one model.")
    args = parser.parse_args()
    if args.llm_model and not args.llm_url:
        parser.error("--llm-model requires --llm-url or A2UI_LLM_URL")
    if args.llm_url:
        try:
            args.llm_url = chat_completions_url(args.llm_url)
        except ValueError as error:
            parser.error(str(error))
    if args.stdio:
        try:
            request = decode_body(sys.stdin.buffer.read(256 * 1024 + 1))
        except (ValueError, UnicodeError) as error:
            sys.stdout.buffer.write(encode_event(str(error), "error"))
            return
        stream_turn(request, sys.stdout.buffer, args.llm_url, args.llm_model)
        return
    with ThreadingHTTPServer(("127.0.0.1", args.port), AgentHandler) as server:
        server.llm_url = args.llm_url
        server.llm_model = args.llm_model
        print(f"Restaurant agent listening at http://127.0.0.1:{server.server_port}/api/agent", flush=True)
        try:
            server.serve_forever()
        except KeyboardInterrupt:
            pass


if __name__ == "__main__":
    main()
