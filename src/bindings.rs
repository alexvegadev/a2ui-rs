use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime};
use regex::Regex;
use serde_json::{Map, Value, json};

use crate::{ProtocolError, Version};

pub(crate) fn absolute_path(scope: &str, path: &str) -> String {
    match path {
        "" => scope.to_owned(),
        "/" => String::new(),
        path if path.starts_with('/') => path.to_owned(),
        path => format!("{scope}/{path}"),
    }
}

fn pointer(path: &str) -> &str {
    if path == "/" { "" } else { path }
}

pub(crate) fn update(
    model: &mut Value,
    path: &str,
    value: Option<Value>,
) -> Result<(), ProtocolError> {
    let path = pointer(path);
    if path.is_empty() {
        *model = value.unwrap_or_else(|| json!({}));
        return Ok(());
    }

    let invalid = || ProtocolError::new(format!("Invalid data model pointer '{path}'."));
    let raw = path.strip_prefix('/').ok_or_else(invalid)?;
    let mut segments = Vec::new();
    for raw in raw.split('/') {
        let mut segment = String::new();
        let mut chars = raw.chars();
        while let Some(ch) = chars.next() {
            segment.push(match ch {
                '~' => match chars.next() {
                    Some('0') => '~',
                    Some('1') => '/',
                    _ => return Err(invalid()),
                },
                other => other,
            });
        }
        segments.push(segment);
        if segments.len() > 128 {
            return Err(invalid());
        }
    }
    write_at(model, &segments, value)
}

fn write_at(
    node: &mut Value,
    segments: &[String],
    value: Option<Value>,
) -> Result<(), ProtocolError> {
    let Some((key, rest)) = segments.split_first() else {
        *node = value.unwrap_or(Value::Null);
        return Ok(());
    };
    match node {
        Value::Object(object) if rest.is_empty() => {
            if let Some(value) = value {
                object.insert(key.clone(), value);
            } else {
                object.remove(key);
            }
        }
        Value::Object(object) => {
            if let Some(child) = object.get_mut(key) {
                write_at(child, rest, value)?;
            } else if let Some(value) = value {
                object.insert(key.clone(), branch(rest, value));
            }
        }
        Value::Array(items) => {
            let index = key
                .parse::<usize>()
                .ok()
                .filter(|index| index.to_string() == *key)
                .ok_or_else(|| ProtocolError::new(format!("Invalid array index '{key}'.")))?;
            if let Some(child) = items.get_mut(index) {
                write_at(child, rest, value)?;
            } else if let Some(value) = value {
                if index > items.len() {
                    return Err(ProtocolError::new(format!(
                        "Array index {index} is out of bounds."
                    )));
                }
                items.push(branch(rest, value));
            }
        }
        _ => {
            if let Some(value) = value {
                *node = branch(segments, value);
            }
        }
    }
    Ok(())
}

fn branch(segments: &[String], value: Value) -> Value {
    segments
        .iter()
        .rev()
        .fold(value, |child, key| json!({ (key): child }))
}

pub(crate) fn text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        value => value.to_string(),
    }
}

pub(crate) struct Evaluator<'a> {
    model: &'a Value,
    scope: &'a str,
    version: Version,
}

impl<'a> Evaluator<'a> {
    pub fn new(model: &'a Value, scope: &'a str, version: Version) -> Self {
        Self {
            model,
            scope,
            version,
        }
    }

    pub fn resolve(&self, value: &Value) -> Value {
        self.resolve_nested(value, 0)
    }

    fn lookup(&self, path: &str) -> &Value {
        self.model
            .pointer(pointer(&absolute_path(self.scope, path)))
            .unwrap_or(&Value::Null)
    }

    fn resolve_nested(&self, value: &Value, depth: usize) -> Value {
        if depth == 64 {
            return Value::Null;
        }
        match value {
            Value::Object(object) => {
                if object.len() == 1
                    && let Some(path) = object.get(self.version.path_key()).and_then(Value::as_str)
                {
                    return self.lookup(path).clone();
                }
                if let Some(call) = object.get(self.version.call_key()).and_then(Value::as_str) {
                    if object
                        .get("catalogId")
                        .or(object.get("@catalogId"))
                        .and_then(Value::as_str)
                        .is_some_and(|catalog| !self.version.supports(catalog))
                    {
                        return Value::Null;
                    }
                    let args = self.resolve_nested(&value["args"], depth + 1);
                    return self.call(call, &args, depth + 1);
                }
                Value::Object(
                    object
                        .iter()
                        .map(|(key, item)| (key.clone(), self.resolve_nested(item, depth + 1)))
                        .collect(),
                )
            }
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item| self.resolve_nested(item, depth + 1))
                    .collect(),
            ),
            value => value.clone(),
        }
    }

    fn call(&self, name: &str, args: &Value, depth: usize) -> Value {
        let value = &args["value"];
        match name {
            "required" => json!(match value {
                Value::Null => false,
                Value::String(value) => !value.is_empty(),
                Value::Array(value) => !value.is_empty(),
                _ => true,
            }),
            "regex" => json!(
                value
                    .as_str()
                    .zip(args["pattern"].as_str())
                    .is_some_and(|(value, pattern)| pattern.len() <= 2_048
                        && Regex::new(pattern).is_ok_and(|regex| regex.is_match(value)))
            ),
            "length" => json!(
                value
                    .as_str()
                    .is_some_and(|value| in_range(value.chars().count() as f64, args))
            ),
            "numeric" => json!(number(value).is_some_and(|value| in_range(value, args))),
            "email" => json!(value.as_str().is_some_and(valid_email)),
            "and" | "or" => {
                let Some(items) = args["values"].as_array() else {
                    return Value::Null;
                };
                let mut values = items.iter().map(|item| item.as_bool() == Some(true));
                json!(if name == "and" {
                    values.all(|value| value)
                } else {
                    values.any(|value| value)
                })
            }
            "not" => value
                .as_bool()
                .map(|value| json!(!value))
                .unwrap_or(Value::Null),
            "formatString" => json!(
                value
                    .as_str()
                    .map(|template| self.interpolate(template, depth + 1))
                    .unwrap_or_default()
            ),
            "formatNumber" => number(value)
                .map(|value| json!(format_number(value, args, 0, 3)))
                .unwrap_or(Value::Null),
            "formatCurrency" => number(value)
                .and_then(|value| {
                    let currency = args["currency"].as_str()?;
                    let (symbol, digits) = match currency {
                        "USD" => ("$", 2),
                        "EUR" => ("€", 2),
                        "GBP" => ("£", 2),
                        "JPY" => ("¥", 0),
                        _ => (currency, 2),
                    };
                    let sign = if value.is_sign_negative() { "-" } else { "" };
                    let separator = if symbol.len() == 3 && symbol == currency {
                        " "
                    } else {
                        ""
                    };
                    Some(json!(format!(
                        "{sign}{symbol}{separator}{}",
                        format_number(value.abs(), args, digits, digits)
                    )))
                })
                .unwrap_or(Value::Null),
            "formatDate" => format_date(value, &args["format"])
                .map(Value::String)
                .unwrap_or(Value::Null),
            "pluralize" => number(value)
                .map(|count| {
                    let category = if count.abs() == 1.0 { "one" } else { "other" };
                    args.get(category).unwrap_or(&args["other"]).clone()
                })
                .unwrap_or(Value::Null),
            _ => Value::Null,
        }
    }

    fn interpolate(&self, mut source: &str, depth: usize) -> String {
        let mut output = String::new();
        if depth >= 32 {
            return output;
        }
        while let Some(start) = source.find("${") {
            let prefix = &source[..start];
            if let Some(prefix) = prefix.strip_suffix('\\') {
                output.push_str(prefix);
                output.push_str("${");
                source = &source[start + 2..];
                continue;
            }
            let body = &source[start + 2..];
            let Some(end) = expression_end(body) else {
                break;
            };
            output.push_str(prefix);
            output.push_str(&text(&self.expression(body[..end].trim(), depth + 1)));
            source = &body[end + 1..];
        }
        output.push_str(source);
        output
    }

    fn expression(&self, expression: &str, depth: usize) -> Value {
        if depth >= 32 {
            return Value::Null;
        }
        if let Some(binding) = expression
            .strip_prefix("${")
            .and_then(|expression| expression.strip_suffix('}'))
        {
            return self.expression(binding.trim(), depth + 1);
        }
        if let Some(literal) = expression
            .strip_prefix('\'')
            .and_then(|expression| expression.strip_suffix('\''))
        {
            return Value::String(self.interpolate(literal, depth + 1));
        }
        if let Ok(literal) = serde_json::from_str(expression) {
            return literal;
        }
        let Some((name, arguments)) = expression.split_once('(') else {
            return self.lookup(expression).clone();
        };
        let Some(arguments) = arguments.strip_suffix(')') else {
            return Value::Null;
        };
        let mut args = Map::new();
        for argument in split_arguments(arguments) {
            let Some((key, value)) = argument.split_once(':') else {
                continue;
            };
            args.insert(
                key.trim().to_owned(),
                self.expression(value.trim(), depth + 1),
            );
        }
        self.call(name.trim(), &Value::Object(args), depth + 1)
    }
}

fn expression_end(source: &str) -> Option<usize> {
    let mut depth = 0;
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in source.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '{' => depth += 1,
            '}' if depth == 0 => return Some(index),
            '}' => depth -= 1,
            _ => {}
        }
    }
    None
}

fn split_arguments(source: &str) -> Vec<&str> {
    let mut arguments = Vec::new();
    let (mut start, mut nested, mut quote, mut escaped) = (0, 0_u32, None, false);
    for (index, ch) in source.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '{' | '[' | '(' => nested += 1,
            '}' | ']' | ')' => nested = nested.saturating_sub(1),
            ',' if nested == 0 => {
                arguments.push(&source[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    arguments.push(&source[start..]);
    arguments
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse().ok())
        .filter(|n| n.is_finite())
}

fn in_range(value: f64, args: &Value) -> bool {
    number(&args["min"]).is_none_or(|min| value >= min)
        && number(&args["max"]).is_none_or(|max| value <= max)
}

fn valid_email(value: &str) -> bool {
    value.split_once('@').is_some_and(|(name, domain)| {
        !name.is_empty()
            && domain
                .split_once('.')
                .is_some_and(|(first, last)| !first.is_empty() && !last.is_empty())
            && !domain.contains('@')
            && !value.chars().any(char::is_whitespace)
    })
}

fn format_number(value: f64, args: &Value, default_min: u64, default_max: u64) -> String {
    let decimals = number(&args["decimals"]).map(|count| count.clamp(0., 20.) as usize);
    let min = decimals.unwrap_or(default_min as usize);
    let max = decimals.unwrap_or(default_max as usize);
    let string = format!("{:.*}", max.max(min), value);
    let (integer, fraction) = string.split_once('.').unwrap_or((&string, ""));
    let fraction = &fraction[..fraction.trim_end_matches('0').len().max(min)];
    let mut result = String::new();
    let digits = integer.strip_prefix('-').unwrap_or(integer);
    if integer.starts_with('-') {
        result.push('-');
    }
    for (index, digit) in digits.chars().enumerate() {
        if index > 0
            && (digits.len() - index).is_multiple_of(3)
            && args["grouping"].as_bool().unwrap_or(true)
        {
            result.push(',');
        }
        result.push(digit);
    }
    if !fraction.is_empty() {
        result.push('.');
        result.push_str(fraction);
    }
    result
}

fn format_date(value: &Value, format: &Value) -> Option<String> {
    let value = value.as_str()?;
    let pattern = format.as_str().unwrap_or("yyyy-MM-dd");
    let mut format = String::new();
    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        let mut count = 1;
        while chars.peek() == Some(&ch) {
            chars.next();
            count += 1;
        }
        let token = match (ch, count) {
            ('y', 2) => "%y",
            ('y', _) => "%Y",
            ('M', 1) => "%-m",
            ('M', 2) => "%m",
            ('M', 3) => "%b",
            ('M', _) => "%B",
            ('d', 1) => "%-d",
            ('d', _) => "%d",
            ('E', 1..=3) => "%a",
            ('E', _) => "%A",
            ('H', 1) => "%-H",
            ('H', _) => "%H",
            ('h', 1) => "%-I",
            ('h', _) => "%I",
            ('m', 1) => "%-M",
            ('m', _) => "%M",
            ('s', 1) => "%-S",
            ('s', _) => "%S",
            ('a', _) => "%p",
            ('%', _) => "%%",
            _ => {
                format.extend(std::iter::repeat_n(ch, count));
                continue;
            }
        };
        format.push_str(token);
    }
    fn formatted(value: impl std::fmt::Display) -> Option<String> {
        use std::fmt::Write as _;
        let mut out = String::new();
        write!(out, "{value}").ok().map(|_| out)
    }
    if let Ok(datetime) = DateTime::parse_from_rfc3339(value) {
        formatted(datetime.format(&format))
    } else if let Ok(datetime) = value.parse::<NaiveDateTime>() {
        formatted(datetime.format(&format))
    } else if let Ok(date) = value.parse::<NaiveDate>() {
        formatted(date.format(&format))
    } else {
        value
            .parse::<NaiveTime>()
            .ok()
            .and_then(|time| formatted(time.format(&format)))
    }
}
