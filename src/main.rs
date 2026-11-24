mod agent;
mod media;

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use a2ui_rs::{A2uiView, ClientEvent};
use gpui_kit::component::{
    ActiveTheme, Disableable, Sizable, Theme, ThemeMode,
    button::{Button, ButtonVariants},
    input::{Input as PromptInput, InputEvent, InputState},
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

const DEMO: &str = include_str!("../examples/reservation.jsonl");

enum Source {
    Demo,
    File(PathBuf),
    Stdin,
}

enum Mode {
    Playback(Source),
    Agent(agent::AgentSession),
}

const USAGE: &str = "Usage: a2ui-rs [FILE.jsonl | - | --python-agent | --agent [URL]]\n\nWith no arguments, play the built-in reservation example.\nUse - to stream JSONL from stdin. Actions are written to stdout.\n--python-agent runs the local demo agent using Python on PATH.\n--agent connects to an HTTP/SSE agent endpoint.";

fn main() {
    let args = std::env::args_os().skip(1).collect::<Vec<OsString>>();
    let mode = match args.as_slice() {
        [] => Mode::Playback(Source::Demo),
        [flag] if flag == "-h" || flag == "--help" => {
            println!("{USAGE}");
            return;
        }
        [flag] if flag == "-" => Mode::Playback(Source::Stdin),
        [flag] if flag == "--python-agent" => {
            Mode::Agent(agent::AgentSession::new(agent::Transport::Python))
        }
        [flag] if flag == "--agent" => agent_mode(agent::DEFAULT_ENDPOINT),
        [flag, endpoint] if flag == "--agent" => agent_mode(&endpoint.to_string_lossy()),
        [file] => Mode::Playback(Source::File(PathBuf::from(file))),
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    let media_policy = media::MediaPolicy::from_env().unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2);
    });
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            cx.set_http_client(Arc::new(media::ImageHttpClient::new(media_policy.clone())));
            cx.set_quit_mode(QuitMode::LastWindowClosed);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1_075.), px(770.)),
                    cx,
                ))),
                titlebar: Some(TitlebarOptions {
                    title: Some("A2UI Renderer".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            if let Err(error) = gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| Desktop::new(mode, media_policy, window, cx))
            }) {
                eprintln!("Could not open the renderer: {error}");
                cx.quit();
                return;
            }
            cx.activate(true);
        });
}

fn agent_mode(endpoint: &str) -> Mode {
    match agent::AgentSession::http(endpoint) {
        Ok(session) => Mode::Agent(session),
        Err(error) => {
            eprintln!("Invalid agent endpoint: {error}");
            std::process::exit(2);
        }
    }
}

enum Input {
    Line(usize, String),
    Error(String),
}

fn feed(source: Source, sender: async_channel::Sender<Input>) {
    fn read(input: impl BufRead, sender: &async_channel::Sender<Input>, paced: bool) {
        for (index, line) in input.lines().enumerate() {
            let message = match line {
                Ok(line) if line.trim().is_empty() => continue,
                Ok(line) => Input::Line(index + 1, line),
                Err(error) => {
                    sender
                        .send_blocking(Input::Error(format!("Could not read input: {error}")))
                        .ok();
                    break;
                }
            };
            if sender.send_blocking(message).is_err() {
                break;
            }
            if paced {
                std::thread::sleep(Duration::from_millis(220));
            }
        }
    }
    match source {
        Source::Demo => read(DEMO.as_bytes(), &sender, true),
        Source::Stdin => read(io::stdin().lock(), &sender, false),
        Source::File(path) => match File::open(&path) {
            Ok(file) => read(BufReader::new(file), &sender, false),
            Err(error) => {
                sender
                    .send_blocking(Input::Error(format!("{}: {error}", path.display())))
                    .ok();
            }
        },
    }
}

struct Desktop {
    client: Entity<A2uiView>,
    label: SharedString,
    received: usize,
    errors: usize,
    last_error: Option<SharedString>,
    last_action: Option<SharedString>,
    reading: bool,
    chat: Option<AgentChat>,
    _actions: Subscription,
    _input: Option<Task<()>>,
}

struct AgentChat {
    session: agent::AgentSession,
    prompt: Entity<InputState>,
    response: String,
    _prompt: Subscription,
}

impl Desktop {
    fn new(
        mode: Mode,
        media_policy: media::MediaPolicy,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let client =
            cx.new(|_| A2uiView::with_url_policy(move |kind, url| media_policy.allows(kind, url)));
        let actions = cx.subscribe(&client, |this, _, action: &ClientEvent, cx| {
            if let Ok(json) = serde_json::to_string(action) {
                println!("{json}");
            }
            this.last_action = serde_json::to_string_pretty(action).ok().map(Into::into);
            this.request_turn(None, Some(action), cx);
            cx.notify();
        });
        let (label, chat, input) = match mode {
            Mode::Playback(source) => {
                let label = match &source {
                    Source::Demo => "Reservation demo".into(),
                    Source::File(path) => path.to_string_lossy().into_owned().into(),
                    Source::Stdin => "Standard input".into(),
                };
                let (sender, receiver) = async_channel::bounded(64);
                std::thread::spawn(move || feed(source, sender));
                let input = cx.spawn(async move |view, cx| {
                    while let Ok(item) = receiver.recv().await {
                        if view.update(cx, |this, cx| this.receive(item, cx)).is_err() {
                            return;
                        }
                    }
                    view.update(cx, |this, cx| {
                        this.reading = false;
                        cx.notify();
                    })
                    .ok();
                });
                (label, None, Some(input))
            }
            Mode::Agent(session) => {
                let prompt = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder("Find Italian restaurants or book a table for 2…")
                });
                let subscription = cx.subscribe_in(
                    &prompt,
                    window,
                    |this, _, event: &InputEvent, window, cx| {
                        if matches!(event, InputEvent::PressEnter { .. }) {
                            this.submit_prompt(window, cx);
                        }
                    },
                );
                let label = session.label().into();
                (
                    label,
                    Some(AgentChat {
                        session,
                        prompt,
                        response: String::new(),
                        _prompt: subscription,
                    }),
                    None,
                )
            }
        };
        Self {
            client,
            label,
            received: 0,
            errors: 0,
            last_error: None,
            last_action: None,
            reading: input.is_some(),
            chat,
            _actions: actions,
            _input: input,
        }
    }

    fn submit_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.chat.as_ref().map(|chat| chat.prompt.clone()) else {
            return;
        };
        let value = prompt.read(cx).value();
        if !value.trim().is_empty() && self.request_turn(Some(value.trim()), None, cx) {
            prompt.update(cx, |state, cx| state.set_value("", window, cx));
        }
    }

    fn request_turn(
        &mut self,
        prompt: Option<&str>,
        action: Option<&ClientEvent>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(chat) = self.chat.as_mut() else {
            return false;
        };
        if self.reading {
            return false;
        }
        let body = chat
            .session
            .request(self.client.read(cx).document(), prompt, action);
        let transport = chat.session.transport.clone();
        chat.response.clear();
        self.last_error = None;
        self.reading = true;
        let (sender, receiver) = async_channel::bounded(32);
        std::thread::spawn(move || agent::stream(transport, body, sender));
        cx.spawn(async move |view, cx| {
            while let Ok(event) = receiver.recv().await {
                if view
                    .update(cx, |this, cx| this.receive_agent(event, cx))
                    .is_err()
                {
                    return;
                }
            }
            view.update(cx, |this, cx| {
                this.reading = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
        true
    }

    fn receive_agent(&mut self, event: agent::AgentEvent, cx: &mut Context<Self>) {
        let Some(chat) = self.chat.as_mut() else {
            return;
        };
        match event {
            agent::AgentEvent::Text(delta) => chat.response.push_str(&delta),
            agent::AgentEvent::Ui(message) => {
                let line = serde_json::Value::Object(message).to_string();
                self.receive(Input::Line(self.received + self.errors + 1, line), cx);
            }
            agent::AgentEvent::Error(error) => {
                self.receive(Input::Error(error), cx);
            }
        }
        cx.notify();
    }

    fn receive(&mut self, item: Input, cx: &mut Context<Self>) {
        let error = match item {
            Input::Line(index, line) => {
                match self
                    .client
                    .update(cx, |client, cx| client.apply_json(&line, cx))
                {
                    Ok(()) => {
                        self.received += 1;
                        None
                    }
                    Err(error) => Some(format!("Line {index}: {error}")),
                }
            }
            Input::Error(error) => Some(error),
        };
        if let Some(error) = error {
            eprintln!("{error}");
            self.errors += 1;
            self.last_error = Some(error.into());
        }
        cx.notify();
    }
}

impl Render for Desktop {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let border = theme.border;
        let panel = rgb(0x171b24);
        let copy = self.last_action.clone();
        let status = match (self.chat.is_some(), self.reading) {
            (true, true) => "Agent responding",
            (true, false) => "Ready",
            (false, true) => "Receiving",
            (false, false) => "Input complete",
        };
        let composer = self.chat.as_ref().map(|chat| {
            div()
                .px_5()
                .py_3()
                .bg(panel)
                .border_t_1()
                .border_color(border)
                .flex()
                .flex_col()
                .gap_2()
                .when(self.received == 0 && !self.reading, |this| {
                    this.child(
                        Button::new("example-prompt")
                            .small()
                            .ghost()
                            .label("Find Italian restaurants")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.request_turn(Some("Find Italian restaurants"), None, cx);
                            })),
                    )
                })
                .when(!chat.response.is_empty(), |this| {
                    this.child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child(chat.response.clone()),
                    )
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            PromptInput::new(&chat.prompt)
                                .accessibility_id("Agent prompt")
                                .flex_1()
                                .min_w_0(),
                        )
                        .child(
                            Button::new("send-prompt")
                                .primary()
                                .disabled(self.reading)
                                .label("Send")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.submit_prompt(window, cx)
                                })),
                        ),
                )
        });
        div().size_full().flex().flex_col().bg(rgb(0x10131a)).text_color(theme.foreground)
            .child(div().h(px(65.)).flex_shrink_0().px_6().border_b_1().border_color(border)
                .flex().items_center().justify_between().bg(panel)
                .child(div().flex().items_center().gap_3()
                    .child(div().size(px(32.)).rounded_md().bg(rgb(0xb5a4fc)).text_color(rgb(0x19152a))
                        .flex().items_center().justify_center().font_weight(FontWeight::BOLD).child("A"))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("A2UI Renderer")))
                .child(div().flex().gap_3().items_center().text_sm().text_color(muted)
                    .child(self.label.clone())
                    .child(div().rounded_md().px_2().py_1().border_1().border_color(border)
                        .child(status))))
            .child(div().flex_1().min_h_0().flex()
                .child(div().flex_1().min_w_0().min_h_0().flex().flex_col()
                    .child(div().flex_1().min_h_0().child(self.client.clone()))
                    .children(composer))
                .child(div().id("protocol-inspector").w(px(340.)).flex_shrink_0().h_full()
                    .overflow_y_scroll().border_l_1().border_color(border).bg(panel).p_5()
                    .child(div().flex().flex_col().gap_4()
                        .child(div().flex().flex_col().gap_1()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child("Protocol activity"))
                            .child(div().text_sm().text_color(muted).child(
                                format!("{} messages · {} errors", self.received, self.errors),
                            )))
                        .when_some(self.last_error.clone(), |panel, error| panel.child(
                            div().p_3().rounded_md().bg(theme.danger.opacity(0.12))
                                .text_sm().text_color(theme.danger).child(error),
                        ))
                        .child(div().border_t_1().border_color(border).pt_4().flex().flex_col().gap_3()
                            .child(div().flex().items_center().justify_between()
                                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Last action"))
                                .when_some(copy, |this, data| this.child(
                                    Button::new("copy-action").ghost().small().label("Copy")
                                        .on_click(move |_, _, cx| cx.write_to_clipboard(
                                            ClipboardItem::new_string(data.to_string())
                                        )),
                                )))
                            .child(match self.last_action.clone() {
                                Some(json) => div().id("action-json").overflow_x_scroll().p_3()
                                    .rounded_md().bg(rgb(0x10131a)).text_xs()
                                    .font_family("Consolas").whitespace_nowrap().child(json)
                                    .into_any_element(),
                                None => div().text_sm().text_color(muted)
                                    .child("Actions appear here when you interact with a surface.")
                                    .into_any_element(),
                            })))))
    }
}
