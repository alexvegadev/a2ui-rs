use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use a2ui_rs::{BASIC_CATALOG_V1, ClientEvent, Document, Version};
use serde_json::{Map, Value, json};
use url::Url;

pub const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:8765/api/agent";
const MAX_EVENT: u64 = 1024 * 1024;
const DEMO_AGENT: &str = include_str!("../examples/python_agent.py");

#[derive(Clone)]
pub enum Transport {
    Http(Url),
    Python,
}

pub struct AgentSession {
    pub transport: Transport,
    session_id: String,
}

impl AgentSession {
    pub fn http(endpoint: &str) -> Result<Self, String> {
        let mut endpoint = Url::parse(endpoint).map_err(|error| error.to_string())?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || !endpoint.has_host()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.fragment().is_some()
        {
            return Err("Expected an HTTP(S) agent URL without credentials or a fragment.".into());
        }
        if endpoint.path() == "/" {
            endpoint.set_path("/api/agent");
        }
        Ok(Self::new(Transport::Http(endpoint)))
    }

    pub fn new(transport: Transport) -> Self {
        Self {
            transport,
            session_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    pub fn label(&self) -> String {
        match &self.transport {
            Transport::Http(endpoint) => {
                format!("Agent · {}", endpoint.host_str().unwrap_or_default())
            }
            Transport::Python => "Python agent · Local demo".into(),
        }
    }

    pub fn request(
        &self,
        document: &Document,
        prompt: Option<&str>,
        action: Option<&ClientEvent>,
    ) -> Value {
        let models: Map<String, Value> = document
            .surfaces()
            .iter()
            .filter(|surface| surface.version == Version::V1 && surface.send_data_model)
            .map(|surface| (surface.id.clone(), surface.data_model().clone()))
            .collect();
        let surfaces = document
            .surfaces()
            .iter()
            .map(|surface| &surface.id)
            .collect::<Vec<_>>();
        let mut request = json!({
            "sessionId": self.session_id,
            "metadata": {
                "a2uiRendererCapabilities": { "1.0": { "supportedCatalogIds": [BASIC_CATALOG_V1] } },
                "a2uiRendererDataModel": { "version": "1.0", "surfaces": models }
            },
            "surfaces": surfaces
        });
        if let Some(action) = action {
            request["message"] = json!(action);
        } else if let Some(prompt) = prompt {
            request["prompt"] = json!(prompt);
        }
        request
    }
}

pub enum AgentEvent {
    Ui(Map<String, Value>),
    Text(String),
    Error(String),
}

pub fn stream(transport: Transport, request: Value, send: async_channel::Sender<AgentEvent>) {
    let body = request.to_string();
    let deliver = |event| send.send_blocking(event).is_ok();
    let result = match transport {
        Transport::Http(endpoint) => stream_http(&endpoint, &body, deliver),
        Transport::Python => stream_python(&body, deliver),
    };
    if let Err(error) = result {
        send.send_blocking(AgentEvent::Error(error)).ok();
    }
}

fn stream_http(
    endpoint: &Url,
    body: &str,
    deliver: impl FnMut(AgentEvent) -> bool,
) -> Result<(), String> {
    let client: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .timeout_global(Some(Duration::from_secs(120)))
        .build()
        .into();
    let response = client
        .post(endpoint.as_str())
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .send(body.as_bytes())
        .map_err(|error| format!("Agent request failed: {error}"))?;
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !response.status().is_success()
        || content_type.split(';').next() != Some("text/event-stream")
    {
        return Err("The agent did not return a successful SSE response.".into());
    }
    read_events(BufReader::new(response.into_body().into_reader()), deliver)
}

fn stream_python(body: &str, deliver: impl FnMut(AgentEvent) -> bool) -> Result<(), String> {
    let mut child = Command::new("python")
        .args(["-B", "-c", DEMO_AGENT, "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| {
            format!("Could not start the Python demo. Check that python is on PATH. {error}")
        })?;
    let input = child.stdin.take().expect("Python stdin is piped");
    let output = child.stdout.take().expect("Python stdout is piped");
    let result =
        write_request(input, body).and_then(|()| read_events(BufReader::new(output), deliver));
    if result.is_err() {
        child.kill().ok();
    }
    let status = child
        .wait()
        .map_err(|error| format!("Could not wait for Python: {error}"))?;
    result?;
    if !status.success() {
        return Err(format!("The Python agent exited with {status}."));
    }
    Ok(())
}

fn write_request(mut input: impl Write, body: &str) -> Result<(), String> {
    input
        .write_all(body.as_bytes())
        .map_err(|error| format!("Could not send turn to Python: {error}"))
}

fn read_events(
    mut input: impl BufRead,
    mut deliver: impl FnMut(AgentEvent) -> bool,
) -> Result<(), String> {
    let mut line = String::new();
    let mut frame = String::new();
    let mut event_name = String::new();
    loop {
        line.clear();
        let size = (&mut input)
            .take(MAX_EVENT + 1)
            .read_line(&mut line)
            .map_err(|error| format!("Could not read the agent stream: {error}"))?;
        if size == 0 {
            return if frame.is_empty() {
                Ok(())
            } else {
                Err("Agent stream ended in the middle of an SSE event.".into())
            };
        }
        if size as u64 > MAX_EVENT {
            return Err("Agent SSE line exceeded 1 MiB.".into());
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if let Some(value) = line.strip_prefix("data:") {
            let value = value.strip_prefix(' ').unwrap_or(value);
            if (frame.len() + value.len() + 1) as u64 > MAX_EVENT {
                return Err("Agent SSE event exceeded 1 MiB.".into());
            }
            frame.push_str(value);
            frame.push('\n');
        } else if let Some(name) = line.strip_prefix("event:") {
            event_name.clear();
            event_name.push_str(name.trim());
        } else if line.is_empty() {
            if !frame.is_empty()
                && let Some(event) = decode(&event_name, &frame)
                    .map_err(|error| format!("Invalid agent event: {error}"))?
                && !deliver(event)
            {
                return Ok(());
            }
            frame.clear();
            event_name.clear();
        }
    }
}

fn decode(name: &str, frame: &str) -> Result<Option<AgentEvent>, io::Error> {
    Ok(match name {
        "" | "message" => Some(AgentEvent::Ui(serde_json::from_str(frame)?)),
        "text" => Some(AgentEvent::Text(serde_json::from_str(frame)?)),
        "error" => Some(AgentEvent::Error(serde_json::from_str(frame)?)),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_reads_raw_a2ui_frames_and_rejects_partial_events() {
        let stream = concat!(
            ": heartbeat\r\n\r\n",
            "data: {\"version\":\"1.0\",\r\n",
            "data: \"deleteSurface\":{\"surfaceId\":\"s-1\"}}\r\n\r\n",
            "event: text\ndata: \"finished\"\n\n"
        );
        let mut events = Vec::new();
        read_events(stream.as_bytes(), |event| {
            events.push(event);
            true
        })
        .unwrap();
        assert!(
            matches!(events.as_slice(), [AgentEvent::Ui(ui), AgentEvent::Text(text)]
            if ui["deleteSurface"]["surfaceId"] == "s-1" && text == "finished")
        );
        assert!(read_events(b"data: {\"version\":\"1.0\"}\n".as_slice(), |_| true).is_err());
    }

    #[test]
    fn agent_request_contains_only_opted_in_models_and_negotiates_basic_catalog() {
        let session = AgentSession::http("http://127.0.0.1:8765").unwrap();
        assert!(matches!(&session.transport, Transport::Http(url) if url.path() == "/api/agent"));
        let mut document = Document::default();
        for (id, sync) in [("editable", true), ("private", false)] {
            document
                .apply_json(
                    &json!({
                        "version":"1.0", "createSurface": {
                            "surfaceId":id, "catalogId":BASIC_CATALOG_V1,
                            "sendDataModel":sync, "dataModel":{"guests":2}
                        }
                    })
                    .to_string(),
                )
                .unwrap();
        }
        let request = session.request(&document, Some("find a table"), None);
        assert_eq!(request["sessionId"], session.session_id);
        assert_eq!(request["prompt"], "find a table");
        assert!(request.get("message").is_none());
        assert_eq!(
            request["metadata"]["a2uiRendererDataModel"]["surfaces"],
            json!({"editable":{"guests":2}})
        );
        assert_eq!(
            request["metadata"]["a2uiRendererCapabilities"]["1.0"]["supportedCatalogIds"][0],
            BASIC_CATALOG_V1
        );
    }

    #[test]
    #[ignore = "requires Python on PATH"]
    fn python_round_trip() {
        let mut document = Document::default();
        let session = AgentSession::new(Transport::Python);
        let request = session.request(&document, Some("Find Italian restaurants"), None);
        run_turn(&mut document, request);
        let action = document
            .surface("restaurants")
            .unwrap()
            .action("select", "/restaurants/0")
            .unwrap();
        assert_eq!(action.action.context["restaurantId"], "olive");
        let request = session.request(&document, None, Some(&action));
        assert!(request.get("prompt").is_none());
        run_turn(&mut document, request);
        let surface = document.surface_mut("restaurants").unwrap();
        surface
            .write_binding("/booking/guests", "", json!(5))
            .unwrap();
        surface
            .write_binding("/booking/seating", "", json!(["terrace"]))
            .unwrap();
        let action = surface.action("confirm", "").unwrap();
        let request = session.request(&document, None, Some(&action));
        run_turn(&mut document, request);
        let model = document.surface("restaurants").unwrap().data_model();
        let summary = model["confirmation"].as_str().unwrap();
        assert!(summary.contains("Olive & Oak · 5 guests"));
        assert!(summary.ends_with("Terrace"));
    }

    fn run_turn(document: &mut Document, request: Value) {
        stream_python(&request.to_string(), |event| {
            match event {
                AgentEvent::Ui(message) => document.apply(message).unwrap(),
                AgentEvent::Error(error) => panic!("Python agent failed: {error}"),
                AgentEvent::Text(_) => {}
            }
            true
        })
        .unwrap();
    }
}
