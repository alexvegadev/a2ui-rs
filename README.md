# A2UI GPUI renderer

A native Rust client for rendering A2UI surfaces with GPUI. The renderer consumes A2UI messages and emits user actions. The application embedding the renderer owns the connection to the agent.

## Try the Python agent

With Python 3.10 or newer on your PATH, install the demo's SDK dependency and run from the project root:

```powershell
python -m pip install -r examples/requirements.txt
cargo run --locked -- --python-agent
```

Choose **Find Italian restaurants** or type a request. Select **Reserve** on a result, adjust the guest count and seating, then choose **Confirm demo reservation**. The confirmation is another A2UI update from Python and includes the values you submitted. The demo uses fictional restaurants, requires no API key, and runs Python as a subprocess.

The official Python Agent SDK negotiates the A2UI 1.0 Basic Catalog, parses incoming action envelopes, and validates each batch of outbound messages before the server streams it.

### Use a local model

For an OpenAI-compatible server listening on port 9931, start the demo in the same PowerShell session:

```powershell
$env:A2UI_LLM_URL = 'http://127.0.0.1:9931'
cargo run --locked -- --python-agent
```

The agent uses the model ID returned by `/v1/models` when the server lists one model. If the server lists several, set `A2UI_LLM_MODEL` to the ID you want. The model interprets prompts and the Python agent builds and validates the restaurant UI. Without a configured model URL, the demo interprets prompts using local rules.

### Connect over HTTP

Run the demo agent and renderer in separate terminals:

```powershell
python examples/python_agent.py
cargo run --locked -- --agent
```

The client defaults to `http://127.0.0.1:8765/api/agent`. To use the FastAPI example from the [A2UI agent integration guide](https://a2ui.org/guides/agent-integration/), start that example with its own dependencies and model credentials, then run `cargo run --locked -- --agent http://127.0.0.1:8080/api/agent`. Both use `prompt` for text, `message` for an A2UI action, and one raw A2UI message per SSE data frame. The demo also includes optional named text and error frames for feedback.

For the same model configuration in HTTP mode, start Python with `python examples/python_agent.py --llm-url http://127.0.0.1:9931`. It also accepts `--llm-model` and a complete chat-completions URL.

Run `cargo test --locked python_round_trip -- --ignored` to exercise the Rust protocol client and Python process together.

## Run

```powershell
cargo run --locked
```

The built-in reservation form arrives as three messages. Change the seating or the guest count, then choose **Confirm reservation**. The protocol panel shows the outbound action with the values currently in the data model. The same action is printed as a JSON line to stdout.

```powershell
cargo run --locked -- examples/reservation.jsonl
Get-Content examples/reservation.jsonl | cargo run --locked -- -
```

The first command opens a JSONL file. The second reads one A2UI object per line from stdin and renders updates as they arrive. The demo can run locally with the embedded data.

The desktop demo blocks agent-provided remote images and browser navigation by default. Set `A2UI_IMAGE_ORIGIN` or `A2UI_NAVIGATION_ORIGIN` to a trusted HTTP(S) origin to enable the corresponding operation. Each setting matches the scheme, host, and port. The image client uses a request timeout and byte limit and blocks redirects.

## Embed in a GPUI app

Initialize GPUI Kit and create an `Entity<A2uiView>` with `cx.new(|_| A2uiView::default())`. Render the entity in your GPUI layout. Pass each incoming message to `view.update(cx, |view, cx| view.apply_json(line, cx))`. Subscribe to `ClientEvent` on that entity to forward actions to the agent. Serialize the event for the wire payload and use `event.metadata()` for the optional surface data model transport metadata.

`Document` is the protocol state without a window. Use it to apply messages, inspect surfaces, resolve bound values, or test action payloads independently from GPUI.

Use `ProtocolError::path()` to identify the rejected field when validation can locate it. To enable remote images or external links, construct the view with `A2uiView::with_url_policy(|kind, url| ...)` and authorize the destinations separately for `ResourceKind::Image` and `ResourceKind::Navigation`. Apply the same image policy in the host's HTTP client and disable redirects or reauthorize each redirect. Navigation policy applies to the initial URL opened by the renderer.

## Coverage

- A2UI 1.0 surface creation and initialization, component upserts, model updates, and deletion, with structural validation for those messages and the Basic Catalog component definitions. Invalid component upserts are rejected before state changes. The renderer also accepts the v0.9 and v0.9.1 wire forms of these messages.
- Progressive rendering, recursive component lookup, template lists with relative paths, and two-way binding for standard form controls.
- Basic Catalog layouts, text and Markdown, images and icons, tabs, dialogs, native form inputs, local validation, and event actions. Legacy surfaces support agent color and attribution in the surface theme.
- HTTP(S) audio and video targets can open in the system browser when authorized by the host. Formatting uses English conventions and supports common date patterns. Full JSON Schema conformance, custom catalog registration, and v1.0 agent/renderer function RPC are outside the current coverage.

Protocol references: [A2UI messages](https://a2ui.org/concepts/messages/) and [Basic Catalog](https://a2ui.org/specification/v1.0-basic-catalog-implementation-guide/).
