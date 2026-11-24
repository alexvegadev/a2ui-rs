use chrono::{DateTime as OffsetDateTime, NaiveDate, NaiveDateTime, NaiveTime};
use gpui_kit::component::{
    Disableable,
    button::ButtonVariants,
    checkbox::Checkbox,
    date_picker::{DatePicker, DatePickerEvent, DatePickerState, DateTime},
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    radio::Radio,
    slider::{Slider, SliderEvent, SliderState, SliderValue},
    time_field::{TimeField, TimeFieldEvent, TimeFieldState, TimePrecision},
};

use super::*;

impl SurfaceRenderer<'_, '_> {
    pub(super) fn input(&mut self, node: &Component, key: &NodeKey) -> AnyElement {
        let value = self.value(node, key);
        let label = self.string(node, "label", &key.scope);
        let locked = node
            .property("value")
            .get(self.surface.version.call_key())
            .is_some();
        let mut failures = self.surface.failed_checks(node, &key.scope);
        let element = match node.kind.as_str() {
            "CheckBox" => {
                let key = key.clone();
                let checkbox = Checkbox::new(key.id("check"))
                    .label(label)
                    .checked(value.as_bool().unwrap_or(false))
                    .disabled(locked)
                    .on_click(self.cx.listener(move |this, checked: &bool, _, cx| {
                        this.edit(&key, Value::Bool(*checked), cx);
                    }));
                return self.field("", checkbox, failures);
            }
            "TextField" => self.text_field(node, key, &label, value, locked),
            "Slider" => self.slider(node, key, &label, value, locked),
            "ChoicePicker" => self.choices(node, key, value, locked),
            "DateTimeInput" => {
                let date = node.property("enableDate").as_bool().unwrap_or(false);
                let time = node.property("enableTime").as_bool().unwrap_or(false);
                let raw = value.as_str().unwrap_or_default();
                let min = self.string(node, "min", &key.scope);
                let max = self.string(node, "max", &key.scope);
                let comparable = |value| parse_date(value, date, time);
                if let Some(value) = comparable(raw) {
                    if comparable(&min).is_some_and(|min| value < min) {
                        failures.push(format!("Choose a value on or after {min}."));
                    }
                    if comparable(&max).is_some_and(|max| value > max) {
                        failures.push(format!("Choose a value on or before {max}."));
                    }
                }
                self.date_time(key, raw, date, time, locked)
            }
            _ => return self.placeholder("Unsupported input"),
        };
        self.field(label, element, failures)
    }

    fn control<S, E>(
        &mut self,
        key: &NodeKey,
        id: SharedString,
        init: impl FnOnce(&mut Window, &mut Context<S>) -> S,
        change: impl Fn(&S, &E) -> Option<Value> + 'static,
    ) -> Entity<S>
    where
        S: EventEmitter<E> + 'static,
        E: 'static,
    {
        let view = self.cx.weak_entity();
        let key = key.clone();
        let control = self.window.use_keyed_state(id, self.cx, |window, cx| {
            let entity = cx.new(|cx| init(window, cx));
            let subscription = cx.subscribe(&entity, move |_, control, event, cx| {
                if let Some(value) = change(control.read(cx), event) {
                    view.update(cx, |view, cx| view.edit(&key, value, cx)).ok();
                }
            });
            (entity, subscription)
        });
        control.read(self.cx).0.clone()
    }

    fn text_field(
        &mut self,
        node: &Component,
        key: &NodeKey,
        label: &str,
        value: Value,
        locked: bool,
    ) -> AnyElement {
        let value = text(&value);
        let placeholder = self.string(node, "placeholder", &key.scope);
        let variant = node.property("variant").as_str().unwrap_or("shortText");
        if variant == "longText" {
            let state = self.control(
                key,
                key.id(variant),
                |window, cx| {
                    TextareaState::new(window, cx)
                        .rows(3)
                        .placeholder(placeholder.clone())
                        .default_value(value.clone())
                },
                |state, event| retype_text(state.value().as_ref(), event),
            );
            if state.read(self.cx).value().as_ref() != value {
                state.update(self.cx, |state, cx| state.set_value(value, self.window, cx));
            }
            Textarea::new(&state)
                .disabled(locked)
                .w_full()
                .into_any_element()
        } else {
            let state = self.control(
                key,
                key.id(variant),
                |window, cx| {
                    let state = InputState::new(window, cx)
                        .masked(variant == "obscured")
                        .placeholder(placeholder.clone())
                        .default_value(value.clone());
                    if variant == "number" {
                        state.validate(|value, _| {
                            value
                                .bytes()
                                .all(|byte| byte.is_ascii_digit() || b"+-.eE".contains(&byte))
                        })
                    } else {
                        state
                    }
                },
                |state, event| retype_text(state.value().as_ref(), event),
            );
            if state.read(self.cx).value().as_ref() != value {
                state.update(self.cx, |state, cx| state.set_value(value, self.window, cx));
            }
            Input::new(&state)
                .disabled(locked)
                .accessibility_id(label.to_owned())
                .w_full()
                .into_any_element()
        }
    }

    fn slider(
        &mut self,
        node: &Component,
        key: &NodeKey,
        label: &str,
        value: Value,
        locked: bool,
    ) -> AnyElement {
        let min = self.resolve(node, "min", &key.scope).as_f64().unwrap_or(0.) as f32;
        let max = self
            .resolve(node, "max", &key.scope)
            .as_f64()
            .unwrap_or(100.) as f32;
        if !min.is_finite() || !max.is_finite() || min >= max || !(max - min).is_finite() {
            return self.placeholder("Slider range unavailable");
        }
        let value = (value.as_f64().unwrap_or(f64::from(min)) as f32).clamp(min, max);
        let step = self
            .resolve(node, "step", &key.scope)
            .as_f64()
            .map(|step| step as f32)
            .filter(|step| *step > 0. && step.is_finite())
            .unwrap_or_else(|| {
                if min.fract() == 0. && max.fract() == 0. {
                    1.
                } else {
                    (max - min) / 100.
                }
            });
        let step = step.max(f32::MIN_POSITIVE).min(max - min);
        let state = self.control(
            key,
            key.id(&format!("slider:{min}:{max}:{step}")),
            |_, _| {
                SliderState::new()
                    .min(min)
                    .max(max)
                    .step(step)
                    .default_value(value)
            },
            |_, event| {
                let SliderEvent::Change(SliderValue::Single(value)) = event else {
                    return None;
                };
                serde_json::to_value(value).ok()
            },
        );
        if state.read(self.cx).value() != SliderValue::Single(value) {
            state.update(self.cx, |state, cx| state.set_value(value, self.window, cx));
        }
        div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                Slider::new(&state)
                    .accessibility_label(label.to_owned())
                    .disabled(locked)
                    .flex_1(),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(self.cx.theme().muted_foreground)
                    .child(value.to_string()),
            )
            .into_any_element()
    }

    fn choices(
        &mut self,
        node: &Component,
        key: &NodeKey,
        value: Value,
        locked: bool,
    ) -> AnyElement {
        let selected: Vec<_> = value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        let exclusive = node.property("variant") == "mutuallyExclusive";
        let chips = node.property("displayStyle") == "chips";
        let limit = node
            .property("maxAllowedSelections")
            .as_u64()
            .unwrap_or(u64::MAX);
        let filter = node
            .property("filterable")
            .as_bool()
            .unwrap_or(false)
            .then(|| {
                let view = self.cx.weak_entity();
                let filter =
                    self.window
                        .use_keyed_state(key.id("filter"), self.cx, |window, cx| {
                            let state = cx.new(|cx| {
                                InputState::new(window, cx).placeholder("Filter options")
                            });
                            let subscription =
                                cx.subscribe(&state, move |_, _, event: &InputEvent, cx| {
                                    if matches!(event, InputEvent::Change) {
                                        view.update(cx, |_, cx| cx.notify()).ok();
                                    }
                                });
                            (state, subscription)
                        });
                filter.read(self.cx).0.clone()
            });
        let query = filter
            .as_ref()
            .map(|state| state.read(self.cx).value().to_lowercase())
            .unwrap_or_default();
        let options = node
            .property("options")
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(|(index, option)| {
                let label = text(&self.surface.resolve(&option["label"], &key.scope));
                if !label.to_lowercase().contains(&query) {
                    return None;
                }
                let option = self.surface.resolve(&option["value"], &key.scope);
                let option = option.as_str()?;
                let checked = selected.iter().any(|value| value == option);
                let mut next = selected.clone();
                let disabled = locked
                    || !checked && (limit == 0 || !exclusive && selected.len() as u64 >= limit);
                if checked {
                    if !exclusive {
                        next.retain(|item| item != option);
                    }
                } else {
                    if exclusive {
                        next.clear();
                    }
                    next.push(option.to_owned());
                }
                let key = key.clone();
                let id = key.id(&format!("choice:{index}"));
                let listener = move |view: &mut A2uiView, cx: &mut Context<A2uiView>| {
                    view.edit(&key, serde_json::json!(next), cx);
                };
                Some(if chips {
                    Button::new(id)
                        .label(label)
                        .disabled(disabled)
                        .small()
                        .map(|button| if checked { button.primary() } else { button })
                        .on_click(self.cx.listener(move |this, _, _, cx| listener(this, cx)))
                        .into_any_element()
                } else if exclusive {
                    Radio::new(id)
                        .label(label)
                        .checked(checked)
                        .disabled(disabled)
                        .on_click(
                            self.cx
                                .listener(move |this, _: &bool, _, cx| listener(this, cx)),
                        )
                        .into_any_element()
                } else {
                    Checkbox::new(id)
                        .label(label)
                        .checked(checked)
                        .disabled(disabled)
                        .on_click(
                            self.cx
                                .listener(move |this, _: &bool, _, cx| listener(this, cx)),
                        )
                        .into_any_element()
                })
            })
            .collect::<Vec<_>>();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .children(filter.map(|state| Input::new(&state)))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .map(|this| {
                        if chips {
                            this.flex_wrap()
                        } else {
                            this.flex_col()
                        }
                    })
                    .children(options),
            )
            .into_any_element()
    }

    fn date_time(
        &mut self,
        key: &NodeKey,
        raw: &str,
        date: bool,
        time: bool,
        locked: bool,
    ) -> AnyElement {
        if date {
            let state = self.control(
                key,
                key.id(if time { "date-time" } else { "date" }),
                |window, cx| {
                    let state = DatePickerState::new(window, cx);
                    if time {
                        state.time_precision(TimePrecision::Second)
                    } else {
                        state
                    }
                },
                move |_, event| {
                    let DatePickerEvent::Change(value) = event;
                    let text = value.start().map_or_else(String::new, |value| {
                        if time {
                            value.format("%Y-%m-%dT%H:%M:%S").to_string()
                        } else {
                            value.date().to_string()
                        }
                    });
                    Some(Value::String(text))
                },
            );
            let model = DateTime::Single(parse_date(raw, date, time));
            if state.read(self.cx).date_time() != model {
                state.update(self.cx, |state, cx| {
                    state.set_date_time(model, self.window, cx)
                });
            }
            DatePicker::new(&state).disabled(locked).into_any_element()
        } else if time {
            let state = self.control(
                key,
                key.id("time"),
                |window, cx| TimeFieldState::new(window, cx).precision(TimePrecision::Second),
                |_, event| {
                    let TimeFieldEvent::Change(time) = event;
                    Some(Value::String(time.format("%H:%M:%S").to_string()))
                },
            );
            let value = parse_time(raw).unwrap_or(NaiveTime::MIN);
            if state.read(self.cx).time() != value {
                state.update(self.cx, |state, cx| state.set_time(value, self.window, cx));
            }
            TimeField::new(&state).disabled(locked).into_any_element()
        } else {
            self.placeholder("Date and time input is disabled")
        }
    }
}

fn retype_text(value: &str, event: &InputEvent) -> Option<Value> {
    matches!(event, InputEvent::Change).then(|| Value::String(value.to_owned()))
}

fn parse_time(value: &str) -> Option<NaiveTime> {
    value
        .parse()
        .ok()
        .or_else(|| NaiveTime::parse_from_str(value, "%H:%M").ok())
}

fn parse_date(value: &str, date: bool, time: bool) -> Option<NaiveDateTime> {
    if time && date {
        value.parse().ok().or_else(|| {
            OffsetDateTime::parse_from_rfc3339(value)
                .ok()
                .map(|value| value.naive_local())
        })
    } else if date {
        value
            .parse::<NaiveDate>()
            .ok()
            .map(|value| value.and_time(NaiveTime::MIN))
    } else {
        parse_time(value).map(|time| NaiveDate::MIN.and_time(time))
    }
}
