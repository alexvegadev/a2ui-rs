mod widgets;

use std::collections::HashMap;

use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, Sizable,
    button::{Button, ButtonVariants},
    text::TextView,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::Value;
use url::Url;

use crate::bindings::{absolute_path, text};
use crate::{ClientEvent, Component, Document, ProtocolError, Surface};

/// GPUI view that accepts A2UI messages and emits user actions to its host.
#[derive(Default)]
pub struct A2uiView {
    document: Document,
    state: ViewState,
}

#[derive(Default)]
struct ViewState {
    values: HashMap<NodeKey, Value>,
    tabs: HashMap<NodeKey, String>,
    modal: Option<NodeKey>,
    modal_focus: Option<FocusHandle>,
    return_focus: Option<FocusHandle>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct NodeKey {
    surface: String,
    generation: u64,
    component: String,
    scope: String,
}

impl NodeKey {
    fn new(surface: &Surface, component: &Component, scope: &str) -> Self {
        Self {
            surface: surface.id.clone(),
            generation: surface.generation,
            component: component.id.clone(),
            scope: scope.to_owned(),
        }
    }

    fn id(&self, part: &str) -> SharedString {
        format!(
            "{}:{}:{}:{}:{}",
            self.generation,
            self.component.len(),
            self.component,
            self.scope,
            part
        )
        .into()
    }
}

impl EventEmitter<ClientEvent> for A2uiView {}

impl A2uiView {
    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn apply_json(&mut self, line: &str, cx: &mut Context<Self>) -> Result<(), ProtocolError> {
        self.document.apply_json(line)?;
        let is_live = |key: &NodeKey| {
            self.document.surface(&key.surface).is_some_and(|surface| {
                surface.generation == key.generation && surface.component(&key.component).is_some()
            })
        };
        self.state.values.retain(|key, _| is_live(key));
        self.state.tabs.retain(|key, _| is_live(key));
        if self.state.modal.as_ref().is_some_and(|key| !is_live(key)) {
            self.state.modal = None;
            self.state.modal_focus = None;
            self.state.return_focus = None;
        }
        cx.notify();
        Ok(())
    }

    fn edit(&mut self, key: &NodeKey, value: Value, cx: &mut Context<Self>) {
        let Some(surface) = self.document.surface_mut(&key.surface) else {
            return;
        };
        if surface.generation != key.generation {
            return;
        }
        let Some(component) = surface.component(&key.component) else {
            return;
        };
        let binding = component.property("value");
        if let Some(path) = binding
            .get(surface.version.path_key())
            .and_then(Value::as_str)
        {
            let path = path.to_owned();
            if let Err(error) = surface.write_binding(&path, &key.scope, value) {
                eprintln!("{error}");
            }
        } else if binding.get(surface.version.call_key()).is_none() {
            self.state.values.insert(key.clone(), value);
        }
        cx.notify();
    }

    fn activate(&mut self, key: &NodeKey, cx: &mut Context<Self>) {
        let Some(surface) = self.document.surface(&key.surface) else {
            return;
        };
        if surface.generation != key.generation {
            return;
        }
        if let Some(event) = surface.action(&key.component, &key.scope) {
            cx.emit(event);
            return;
        }
        let Some(node) = surface.component(&key.component) else {
            return;
        };
        if !surface.failed_checks(node, &key.scope).is_empty() {
            return;
        }
        let call = &node.property("action")["functionCall"];
        if call[surface.version.call_key()].as_str() == Some("openUrl") {
            let url = surface.resolve(&call["args"]["url"], &key.scope);
            if let Some(url) = url.as_str().and_then(web_url) {
                cx.open_url(url.as_str());
            }
        }
    }

    fn open_modal(&mut self, key: NodeKey, window: &mut Window, cx: &mut Context<Self>) {
        self.state.return_focus = window.focused(cx);
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        self.state.modal_focus = Some(focus);
        self.state.modal = Some(key);
        cx.notify();
    }

    fn close_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.modal = None;
        self.state.modal_focus = None;
        if let Some(focus) = self.state.return_focus.take() {
            focus.focus(window, cx);
        }
        cx.notify();
    }
}

impl Render for A2uiView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let foreground = theme.foreground;
        let muted = theme.muted_foreground;
        let mut elements = Vec::new();
        let mut overlay = None;
        for surface in self.document.surfaces() {
            let mut renderer = SurfaceRenderer {
                surface,
                state: &self.state,
                window,
                cx,
                budget: 4_096,
            };
            let content = renderer.node("root", "", &mut Vec::new());
            let agent = surface
                .theme
                .get("agentDisplayName")
                .and_then(Value::as_str);
            let avatar = surface
                .theme
                .get("iconUrl")
                .and_then(Value::as_str)
                .and_then(web_url);
            elements.push(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .when_some(agent, |this, name| {
                        this.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_sm()
                                .text_color(muted)
                                .when_some(avatar, |this, url| {
                                    this.child(img(url.to_string()).size(px(20.)).rounded_full())
                                })
                                .child(name.to_owned()),
                        )
                    })
                    .child(content),
            );
            if let Some(key) = self
                .state
                .modal
                .as_ref()
                .filter(|key| key.generation == surface.generation)
            {
                overlay = renderer.modal_overlay(key);
            }
        }
        div()
            .relative()
            .size_full()
            .min_h_0()
            .min_w_0()
            .text_color(foreground)
            .child(
                div()
                    .id("a2ui-surfaces")
                    .size_full()
                    .overflow_y_scroll()
                    .p_6()
                    .child(
                        div()
                            .mx_auto()
                            .w_full()
                            .max_w(px(800.))
                            .flex()
                            .flex_col()
                            .gap_5()
                            .when(elements.is_empty(), |this| {
                                this.child(
                                    div()
                                        .text_sm()
                                        .text_color(muted)
                                        .child("Waiting for an A2UI surface…"),
                                )
                            })
                            .children(elements),
                    ),
            )
            .children(overlay)
    }
}

struct SurfaceRenderer<'a, 'context> {
    surface: &'a Surface,
    state: &'a ViewState,
    window: &'a mut Window,
    cx: &'a mut Context<'context, A2uiView>,
    budget: usize,
}

impl SurfaceRenderer<'_, '_> {
    fn resolve(&self, node: &Component, name: &str, scope: &str) -> Value {
        self.surface.resolve(node.property(name), scope)
    }

    fn string(&self, node: &Component, name: &str, scope: &str) -> String {
        text(&self.resolve(node, name, scope))
    }

    fn value(&self, node: &Component, key: &NodeKey) -> Value {
        let value = node.property("value");
        if !value.is_object()
            && let Some(value) = self.state.values.get(key)
        {
            return value.clone();
        }
        self.resolve(node, "value", &key.scope)
    }

    fn accent(&self) -> Hsla {
        self.surface
            .theme
            .get("primaryColor")
            .and_then(Value::as_str)
            .and_then(|color| color.strip_prefix('#'))
            .filter(|color| color.len() == 6)
            .and_then(|color| u32::from_str_radix(color, 16).ok())
            .map(|color| rgb(color).into())
            .unwrap_or(self.cx.theme().primary)
    }

    fn node<'a>(&mut self, id: &'a str, scope: &str, ancestors: &mut Vec<&'a str>) -> AnyElement
    where
        Self: 'a,
    {
        if self.budget == 0 {
            return div().into_any_element();
        }
        self.budget -= 1;
        if ancestors.len() >= 64 || ancestors.contains(&id) {
            return self.placeholder(format!("Invalid component tree at {id}"));
        }
        let Some(node) = self.surface.component(id) else {
            return self.placeholder(if id == "root" {
                "Waiting for surface content…"
            } else {
                "Loading…"
            });
        };
        if !self.surface.supports_component(node) {
            return self.placeholder(format!("Unsupported catalog for {}", node.kind));
        }
        let key = NodeKey::new(self.surface, node, scope);
        ancestors.push(id);
        let element = match node.kind.as_str() {
            "Text" => self.render_text(node, &key),
            "Image" => self.image(node, scope),
            "Icon" => self.icon(node, scope),
            "AudioPlayer" | "Video" => self.media(node, &key),
            "Row" | "Column" | "List" => self.layout(node, &key, ancestors),
            "Card" => div()
                .w_full()
                .rounded_xl()
                .p_5()
                .border_1()
                .border_color(self.cx.theme().border)
                .bg(self.cx.theme().muted)
                .child(self.child(node, "child", scope, ancestors))
                .into_any_element(),
            "Divider" => div()
                .bg(self.cx.theme().border)
                .map(|this| {
                    if node.property("axis") == "vertical" {
                        this.w(px(1.)).min_h(px(18.)).h_full()
                    } else {
                        this.h(px(1.)).w_full()
                    }
                })
                .into_any_element(),
            "Tabs" => self.tabs(node, &key, ancestors),
            "Modal" => self.modal_trigger(node, &key, ancestors),
            "Button" => self.button(node, &key, ancestors),
            "TextField" | "CheckBox" | "Slider" | "ChoicePicker" | "DateTimeInput" => {
                self.input(node, &key)
            }
            component => self.placeholder(format!("Unsupported component: {component}")),
        };
        ancestors.pop();
        let weight = node
            .property("weight")
            .as_f64()
            .filter(|weight| *weight > 0.0);
        match weight {
            Some(weight) => div()
                .min_w_0()
                .flex_basis(px(0.))
                .flex_grow(weight as f32)
                .child(element)
                .into_any_element(),
            None => element,
        }
    }

    fn child<'a>(
        &mut self,
        node: &'a Component,
        property: &str,
        scope: &str,
        ancestors: &mut Vec<&'a str>,
    ) -> AnyElement
    where
        Self: 'a,
    {
        node.property(property)
            .as_str()
            .map(|id| self.node(id, scope, ancestors))
            .unwrap_or_else(|| div().into_any_element())
    }

    fn placeholder(&self, label: impl Into<SharedString>) -> AnyElement {
        div()
            .text_sm()
            .text_color(self.cx.theme().muted_foreground)
            .child(label.into())
            .into_any_element()
    }

    fn render_text(&self, node: &Component, key: &NodeKey) -> AnyElement {
        let text = self.string(node, "text", &key.scope);
        let size = match node.property("variant").as_str() {
            Some("h1") => 30.,
            Some("h2") => 24.,
            Some("h3") => 20.,
            Some("h4") => 18.,
            Some("h5") => 16.,
            Some("caption" | "label") => 12.,
            _ => 14.,
        };
        let markdown = text.contains(['*', '_', '`', '[', '\n']) || text.starts_with('#');
        div()
            .min_w_0()
            .text_size(px(size))
            .when(
                matches!(
                    node.property("variant").as_str(),
                    Some("h1" | "h2" | "h3" | "h4" | "h5")
                ),
                |this| this.font_weight(FontWeight::SEMIBOLD),
            )
            .when(node.property("variant") == "caption", |this| {
                this.text_color(self.cx.theme().muted_foreground)
            })
            .child(if markdown {
                TextView::markdown(key.id("markdown"), text)
                    .image_source(|uri| {
                        web_url(uri.as_ref())
                            .map(|url| ImageSource::from(url.to_string()))
                            .unwrap_or_else(|| {
                                ImageSource::from(|_: &mut Window, _: &mut App| None)
                            })
                    })
                    .on_link_click(|url, _, _, cx| {
                        if let Some(url) = web_url(url.as_ref()) {
                            cx.open_url(url.as_str());
                        }
                    })
                    .into_any_element()
            } else {
                text.into_any_element()
            })
            .into_any_element()
    }

    fn image(&self, node: &Component, scope: &str) -> AnyElement {
        let Some(url) = web_url(&self.string(node, "url", scope)) else {
            return self.placeholder("Image unavailable");
        };
        let fit = match node.property("fit").as_str() {
            Some("cover") => ObjectFit::Cover,
            Some("fill") => ObjectFit::Fill,
            Some("none") => ObjectFit::None,
            Some("scaleDown") => ObjectFit::ScaleDown,
            _ => ObjectFit::Contain,
        };
        let (width, height) = match node.property("variant").as_str() {
            Some("icon") => (24., 24.),
            Some("avatar") => (48., 48.),
            Some("smallFeature") => (128., 88.),
            Some("mediumFeature") => (240., 144.),
            _ => (440., 220.),
        };
        img(url.to_string())
            .object_fit(fit)
            .w(px(width))
            .max_w_full()
            .h(px(height))
            .map(|this| {
                if node.property("variant") == "avatar" {
                    this.rounded_full()
                } else {
                    this.rounded_md()
                }
            })
            .into_any_element()
    }

    fn icon(&self, node: &Component, scope: &str) -> AnyElement {
        let name = self.string(node, "name", scope);
        let icon = match name.as_str() {
            "accountCircle" => "circle-user",
            "arrowBack" => "arrow-left",
            "arrowForward" => "arrow-right",
            "calendarToday" => "calendar",
            "checkCircle" => "circle-check",
            "close" => "x",
            "error" => "circle-alert",
            "favorite" => "heart",
            "moreHoriz" => "ellipsis",
            "moreVert" => "ellipsis-vertical",
            "person" => "user",
            "phone" => "phone",
            "shoppingCart" => "shopping-cart",
            "warning" => "triangle-alert",
            name => name,
        };
        let path = icon.chars().fold(String::new(), |mut path, ch| {
            if ch.is_ascii_uppercase() {
                path.push('-');
            }
            path.push(ch.to_ascii_lowercase());
            path
        });
        if !path.is_empty()
            && path
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
        {
            Icon::empty()
                .path(format!("icons/{path}.svg"))
                .into_any_element()
        } else {
            self.placeholder("•")
        }
    }

    fn media(&self, node: &Component, key: &NodeKey) -> AnyElement {
        let Some(url) = web_url(&self.string(node, "url", &key.scope)) else {
            return self.placeholder("Media unavailable");
        };
        let label = if node.kind == "Video" {
            "Open video"
        } else {
            "Open audio"
        };
        div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                Button::new(key.id("media"))
                    .label(label)
                    .on_click(move |_, _, cx| cx.open_url(url.as_str())),
            )
            .child(self.string(node, "description", &key.scope))
            .into_any_element()
    }

    fn layout<'a>(
        &mut self,
        node: &'a Component,
        key: &NodeKey,
        ancestors: &mut Vec<&'a str>,
    ) -> AnyElement
    where
        Self: 'a,
    {
        let children = node.property("children");
        let mut elements = Vec::new();
        if let Some(ids) = children.as_array() {
            for id in ids.iter().filter_map(Value::as_str).take(self.budget) {
                elements.push(self.node(id, &key.scope, ancestors));
            }
        } else if let (Some(template), Some(path)) =
            (children["componentId"].as_str(), children["path"].as_str())
        {
            let base = absolute_path(&key.scope, path);
            let length = self
                .surface
                .data_model()
                .pointer(&base)
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            for index in 0..length {
                if self.budget == 0 {
                    break;
                }
                elements.push(self.node(template, &format!("{base}/{index}"), ancestors));
            }
        }
        let horizontal = node.kind == "Row" || node.property("direction") == "horizontal";
        let layout = div()
            .flex()
            .gap_3()
            .min_w_0()
            .map(|this| {
                if horizontal {
                    this.flex_row()
                } else {
                    this.flex_col()
                }
            })
            .map(|this| match node.property("justify").as_str() {
                Some("center") => this.justify_center(),
                Some("end") => this.justify_end(),
                Some("spaceBetween") => this.justify_between(),
                Some("spaceAround") => this.justify_around(),
                Some("spaceEvenly") => this.justify_evenly(),
                _ => this.justify_start(),
            })
            .map(|this| match node.property("align").as_str() {
                Some("start") => this.items_start(),
                Some("center") => this.items_center(),
                Some("end") => this.items_end(),
                _ => this.items_stretch(),
            })
            .children(elements);
        if node.kind == "List" {
            let layout = layout.id(key.id("scroll"));
            if horizontal {
                layout.overflow_x_scroll().into_any_element()
            } else {
                layout
                    .max_h(px(280.))
                    .overflow_y_scroll()
                    .into_any_element()
            }
        } else {
            layout.into_any_element()
        }
    }

    fn button<'a>(
        &mut self,
        node: &'a Component,
        key: &NodeKey,
        ancestors: &mut Vec<&'a str>,
    ) -> AnyElement
    where
        Self: 'a,
    {
        let content = self.child(node, "child", &key.scope, ancestors);
        let failures = self.surface.failed_checks(node, &key.scope);
        let handler_key = key.clone();
        let label = self.button_label(node, &key.scope);
        let button = self
            .styled_button(
                key.id("button"),
                node.property("variant").as_str(),
                !failures.is_empty(),
            )
            .when(!label.is_empty(), |button| {
                button.accessibility_label(label)
            })
            .child(content)
            .on_click(
                self.cx
                    .listener(move |this, _, _, cx| this.activate(&handler_key, cx)),
            );
        self.field("", button, failures)
    }

    fn styled_button(&self, id: SharedString, variant: Option<&str>, disabled: bool) -> Button {
        let button = Button::new(id).disabled(disabled);
        match variant {
            Some("primary") if !disabled => {
                let accent = self.accent();
                let ink = if accent.l > 0.58 {
                    rgb(0x10151c)
                } else {
                    rgb(0xffffff)
                };
                button.primary().bg(accent).text_color(ink)
            }
            Some("primary") => button.primary(),
            Some("borderless") => button.ghost(),
            _ => button,
        }
    }

    fn button_label(&self, node: &Component, scope: &str) -> String {
        if let Some(label) = node.property("@accessibility")["label"].as_str() {
            return label.to_owned();
        }
        let text_node = node
            .property("child")
            .as_str()
            .and_then(|id| self.surface.component(id))
            .unwrap_or(node);
        self.string(text_node, "text", scope)
    }

    fn tabs<'a>(
        &mut self,
        node: &'a Component,
        key: &NodeKey,
        ancestors: &mut Vec<&'a str>,
    ) -> AnyElement
    where
        Self: 'a,
    {
        let Some(items) = node.property("tabItems").as_array() else {
            return div().into_any_element();
        };
        let active = self
            .state
            .tabs
            .get(key)
            .map(String::as_str)
            .filter(|id| items.iter().any(|tab| tab["child"].as_str() == Some(id)))
            .or_else(|| items.first()?.get("child")?.as_str());
        let buttons = items
            .iter()
            .enumerate()
            .filter_map(|(index, tab)| {
                let id = tab["child"].as_str()?.to_owned();
                let selected = active == Some(&id);
                let title = text(&self.surface.resolve(&tab["title"], &key.scope));
                let key = key.clone();
                Some(
                    Button::new(key.id(&format!("tab-{index}")))
                        .label(title)
                        .small()
                        .map(|this| {
                            if selected {
                                this.primary()
                            } else {
                                this.ghost()
                            }
                        })
                        .on_click(self.cx.listener(move |this, _, _, cx| {
                            this.state.tabs.insert(key.clone(), id.clone());
                            cx.notify();
                        })),
                )
            })
            .collect::<Vec<_>>();
        let body = active.map(|id| self.node(id, &key.scope, ancestors));
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().flex().flex_wrap().gap_2().children(buttons))
            .children(body)
            .into_any_element()
    }

    fn modal_trigger<'a>(
        &mut self,
        node: &'a Component,
        key: &NodeKey,
        ancestors: &mut Vec<&'a str>,
    ) -> AnyElement
    where
        Self: 'a,
    {
        let trigger = node
            .property("trigger")
            .as_str()
            .and_then(|id| self.surface.component(id));
        let (child, variant) = match trigger {
            Some(trigger) if trigger.kind == "Button" => (
                self.child(trigger, "child", &key.scope, ancestors),
                trigger.property("variant").as_str(),
            ),
            _ => (self.child(node, "trigger", &key.scope, ancestors), None),
        };
        let label = trigger.map_or_else(
            || "Open dialog".to_owned(),
            |trigger| self.button_label(trigger, &key.scope),
        );
        let key = key.clone();
        self.styled_button(key.id("trigger"), variant, false)
            .when(!label.is_empty(), |button| {
                button.accessibility_label(label)
            })
            .child(child)
            .on_click(
                self.cx
                    .listener(move |this, _, window, cx| this.open_modal(key.clone(), window, cx)),
            )
            .into_any_element()
    }

    fn modal_overlay(&mut self, key: &NodeKey) -> Option<AnyElement> {
        let modal = self.surface.component(&key.component)?;
        let mut ancestors = vec![modal.id.as_str()];
        let body = self.child(modal, "content", &key.scope, &mut ancestors);
        let focus = self.state.modal_focus.as_ref()?;
        Some(
            div()
                .id(key.id("overlay"))
                .absolute()
                .inset_0()
                .p_6()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x000000a0))
                .track_focus(focus)
                .tab_group()
                .on_key_down(self.cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.close_modal(window, cx);
                    }
                }))
                .child(
                    div()
                        .id(key.id("dialog"))
                        .w_full()
                        .max_w(px(540.))
                        .max_h_full()
                        .overflow_y_scroll()
                        .rounded_xl()
                        .border_1()
                        .border_color(self.cx.theme().border)
                        .p_5()
                        .bg(self.cx.theme().background)
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div().flex().justify_end().child(
                                Button::new(key.id("close"))
                                    .small()
                                    .ghost()
                                    .label("Close")
                                    .on_click(self.cx.listener(|this, _, window, cx| {
                                        this.close_modal(window, cx)
                                    })),
                            ),
                        )
                        .child(body),
                )
                .into_any_element(),
        )
    }

    fn field(
        &self,
        label: impl Into<SharedString>,
        element: impl IntoElement,
        failures: Vec<String>,
    ) -> AnyElement {
        let label = label.into();
        div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_2()
            .when(!label.is_empty(), |this| {
                this.child(div().text_sm().font_weight(FontWeight::MEDIUM).child(label))
            })
            .child(element)
            .children(
                failures
                    .into_iter()
                    .filter(|message| !message.is_empty())
                    .map(|message| {
                        div()
                            .text_xs()
                            .text_color(self.cx.theme().danger)
                            .child(message)
                    }),
            )
            .into_any_element()
    }
}

pub(crate) fn web_url(raw: &str) -> Option<Url> {
    let url = Url::parse(raw).ok()?;
    (matches!(url.scheme(), "https" | "http")
        && url.host().is_some()
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(url)
}
