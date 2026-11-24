//! Structural admission checks for the four supported v1.0 surface messages
//! and the v1.0 Basic Catalog. See https://a2ui.org/specification/v1.0/schemas/.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Map, Value};

use crate::{BASIC_CATALOG_V1, Component, ProtocolError};

pub(crate) fn payload(kind: &str, value: &Value) -> Result<(), ProtocolError> {
    let path = format!("/{kind}");
    let fields = value
        .as_object()
        .ok_or_else(|| ProtocolError::at(&path, "Expected a message object."))?;
    let (required, optional): (&[Field], &[Field]) = match kind {
        "createSurface" => (
            &[("surfaceId", String)],
            &[
                ("catalogId", String),
                ("sendDataModel", Bool),
                ("components", Components),
                ("dataModel", Object),
                ("metadata", Metadata),
            ],
        ),
        "updateComponents" => (&[("surfaceId", String), ("components", Components)], &[]),
        "updateDataModel" => (
            &[("surfaceId", String), ("value", Any)],
            &[("path", String)],
        ),
        "deleteSurface" => (&[("surfaceId", String)], &[]),
        _ => return Err(ProtocolError::at(path, "Unsupported A2UI message.")),
    };
    object(fields, &path, required, optional, &[])?;

    if fields["surfaceId"].as_str() == Some("") {
        return Err(ProtocolError::at(
            format!("{path}/surfaceId"),
            "Expected a nonempty surface ID.",
        ));
    }
    if let Some(catalog) = fields.get("catalogId")
        && catalog != BASIC_CATALOG_V1
    {
        return Err(ProtocolError::at(
            format!("{path}/catalogId"),
            "Unsupported catalog.",
        ));
    }
    if let Some(components) = fields.get("components").and_then(Value::as_array) {
        for (i, component) in components.iter().enumerate() {
            let path = format!("{path}/components/{i}");
            let component = component
                .as_object()
                .ok_or_else(|| ProtocolError::at(&path, "Expected a component object."))?;
            for (key, shape) in [("id", Id), ("component", Id)] {
                check_field(component, &path, key, shape)?;
            }
            if let Some(catalog) = component.get("@catalogId")
                && catalog != BASIC_CATALOG_V1
            {
                return Err(ProtocolError::at(
                    format!("{path}/@catalogId"),
                    "Unsupported catalog.",
                ));
            }
        }
    }
    Ok(())
}

type Field = (&'static str, Shape);

#[derive(Clone, Copy)]
enum Shape {
    Any,
    Id,
    String,
    Bool,
    Number,
    PositiveInteger,
    Object,
    Components,
    Metadata,
    Accessibility,
    Expression,
    DynamicString,
    DynamicBoolean,
    DynamicNumber,
    DynamicStringList,
    Choice(&'static [&'static str]),
    ChildList,
    Tabs,
    Options,
    IconName,
    Checks,
    Action,
}
use Shape::*;

const DIRECTION: Shape = Choice(&["vertical", "horizontal"]);
const ALIGN: Shape = Choice(&["start", "center", "end", "stretch"]);
const JUSTIFY: Shape = Choice(&[
    "start",
    "center",
    "end",
    "spaceAround",
    "spaceBetween",
    "spaceEvenly",
    "stretch",
]);
const COMMON: &[Field] = &[
    ("weight", Number),
    ("@metadata", Metadata),
    ("@accessibility", Accessibility),
];

pub(crate) fn component(
    component: &Component,
    surface_catalog: Option<&str>,
    path: &str,
) -> Result<(), ProtocolError> {
    if component.catalog_id.as_deref().or(surface_catalog) != Some(BASIC_CATALOG_V1) {
        return Err(ProtocolError::at(
            format!("{path}/@catalogId"),
            "Expected a supported component or surface catalog.",
        ));
    }
    let (required, optional, checkable): (&[Field], &[Field], _) = match component.kind.as_str() {
        "Text" => (
            &[("text", DynamicString)],
            &[("variant", Choice(&["caption", "body"]))],
            false,
        ),
        "Image" => (
            &[("url", DynamicString)],
            &[
                ("description", DynamicString),
                (
                    "fit",
                    Choice(&["contain", "cover", "fill", "none", "scaleDown"]),
                ),
                (
                    "variant",
                    Choice(&[
                        "icon",
                        "avatar",
                        "smallFeature",
                        "mediumFeature",
                        "largeFeature",
                        "header",
                    ]),
                ),
            ],
            false,
        ),
        "Icon" => (&[("name", IconName)], &[], false),
        "Video" => (
            &[("url", DynamicString)],
            &[("posterUrl", DynamicString)],
            false,
        ),
        "AudioPlayer" => (&[("url", DynamicString)], &[], false),
        "Row" | "Column" => (
            &[("children", ChildList)],
            &[("justify", JUSTIFY), ("align", ALIGN)],
            false,
        ),
        "List" => (
            &[("children", ChildList)],
            &[("direction", DIRECTION), ("align", ALIGN)],
            false,
        ),
        "Card" => (&[("child", Id)], &[], false),
        "Tabs" => (&[("tabs", Tabs)], &[], false),
        "Modal" => (&[("trigger", Id), ("content", Id)], &[], false),
        "Divider" => (&[], &[("axis", DIRECTION)], false),
        "Button" => (
            &[("child", Id), ("action", Action)],
            &[("variant", Choice(&["default", "primary", "borderless"]))],
            true,
        ),
        "TextField" => (
            &[("label", DynamicString), ("value", DynamicString)],
            &[
                ("placeholder", DynamicString),
                (
                    "variant",
                    Choice(&["longText", "number", "shortText", "obscured"]),
                ),
            ],
            true,
        ),
        "CheckBox" => (
            &[("label", DynamicString), ("value", DynamicBoolean)],
            &[],
            true,
        ),
        "ChoicePicker" => (
            &[("options", Options), ("value", DynamicStringList)],
            &[
                ("label", DynamicString),
                (
                    "variant",
                    Choice(&["multipleSelection", "mutuallyExclusive"]),
                ),
                ("displayStyle", Choice(&["checkbox", "chips"])),
                ("filterable", Bool),
            ],
            true,
        ),
        "Slider" => (
            &[("max", Number), ("value", DynamicNumber)],
            &[
                ("label", DynamicString),
                ("min", Number),
                ("steps", PositiveInteger),
            ],
            true,
        ),
        "DateTimeInput" => (
            &[("value", DynamicString)],
            &[
                ("enableDate", Bool),
                ("enableTime", Bool),
                ("min", DynamicString),
                ("max", DynamicString),
                ("label", DynamicString),
            ],
            true,
        ),
        _ => {
            return Err(ProtocolError::at(
                format!("{path}/component"),
                "Unsupported Basic Catalog component.",
            ));
        }
    };
    let checks: &[Field] = if checkable {
        &[("@checks", Checks)]
    } else {
        &[]
    };
    object(
        &component.properties,
        path,
        required,
        optional,
        &[COMMON, checks],
    )
}

fn check_field(
    object: &Map<std::string::String, Value>,
    path: &str,
    key: &str,
    shape: Shape,
) -> Result<(), ProtocolError> {
    match object.get(key) {
        Some(value) if shape.accepts(value) => Ok(()),
        Some(_) => Err(ProtocolError::at(
            format!("{path}/{key}"),
            "Invalid field value.",
        )),
        None => Err(ProtocolError::at(
            format!("{path}/{key}"),
            "Required field is missing.",
        )),
    }
}

fn object(
    fields: &Map<std::string::String, Value>,
    path: &str,
    required: &[Field],
    optional: &[Field],
    common: &[&[Field]],
) -> Result<(), ProtocolError> {
    for &(key, _) in required {
        if !fields.contains_key(key) {
            return Err(ProtocolError::at(
                format!("{path}/{key}"),
                "Required field is missing.",
            ));
        }
    }
    for (key, value) in fields {
        let shape = required
            .iter()
            .chain(optional)
            .chain(common.iter().flat_map(|fields| fields.iter()))
            .find_map(|&(field, shape)| (field == key).then_some(shape));
        let key = key.replace('~', "~0").replace('/', "~1");
        match shape {
            Some(shape) if shape.accepts(value) => {}
            Some(_) => {
                return Err(ProtocolError::at(
                    format!("{path}/{key}"),
                    "Invalid field value.",
                ));
            }
            None => {
                return Err(ProtocolError::at(
                    format!("{path}/{key}"),
                    "Unexpected field.",
                ));
            }
        }
    }
    Ok(())
}

impl Shape {
    fn accepts(self, value: &Value) -> bool {
        match self {
            Any => true,
            Id => value.as_str().is_some_and(identifier),
            String => value.is_string(),
            Bool => value.is_boolean(),
            Number => value.is_number(),
            PositiveInteger => value.as_u64().is_some_and(|number| number > 0),
            Object => value.is_object(),
            Components => value.as_array().is_some_and(|items| !items.is_empty()),
            Metadata => {
                fields(value, &[], &[("extensions", Object)])
                    && value["extensions"]
                        .as_object()
                        .is_none_or(|extensions| extensions.keys().all(|key| identifier(key)))
            }
            Accessibility => fields(
                value,
                &[],
                &[
                    ("label", DynamicString),
                    ("description", DynamicString),
                    ("live", Choice(&["off", "polite", "assertive"])),
                    ("hidden", DynamicBoolean),
                ],
            ),
            Expression => binding(value) || function(value),
            DynamicString => value.is_string() || Expression.accepts(value),
            DynamicBoolean => value.is_boolean() || Expression.accepts(value),
            DynamicNumber => value.is_number() || Expression.accepts(value),
            DynamicStringList => array(value, Value::is_string) || Expression.accepts(value),
            Choice(variants) => value
                .as_str()
                .is_some_and(|variant| variants.contains(&variant)),
            ChildList => {
                array(value, |item| Id.accepts(item))
                    || fields(value, &[("componentId", Id), ("path", String)], &[])
            }
            Tabs => {
                Components.accepts(value)
                    && array(value, |tab| {
                        fields(tab, &[("title", DynamicString), ("child", Id)], &[])
                    })
            }
            Options => array(value, |option| {
                fields(option, &[("label", DynamicString), ("value", String)], &[])
            }),
            IconName => {
                value.is_string()
                    || binding(value)
                    || fields(value, &[("svgPath", DynamicString)], &[])
            }
            Checks => array(value, |rule| {
                fields(rule, &[("condition", Expression)], &[("message", String)])
            }),
            Action => {
                fields(value, &[("functionCall", Expression)], &[])
                    && function(&value["functionCall"])
                    || (value.as_object().is_some_and(|action| action.len() == 1)
                        && fields(
                            &value["event"],
                            &[("name", String)],
                            &[("userMessage", DynamicString), ("context", Object)],
                        ))
            }
        }
    }
}

fn identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || unicode_ident::is_xid_start(first))
        && chars.all(unicode_ident::is_xid_continue)
}

fn fields(value: &Value, required: &[Field], optional: &[Field]) -> bool {
    value.as_object().is_some_and(|value| {
        required
            .iter()
            .all(|&(key, shape)| value.get(key).is_some_and(|value| shape.accepts(value)))
            && value.iter().all(|(key, value)| {
                required
                    .iter()
                    .chain(optional)
                    .any(|&(field, shape)| field == key && shape.accepts(value))
            })
    })
}

fn array(value: &Value, item: impl Fn(&Value) -> bool) -> bool {
    value.as_array().is_some_and(|items| items.iter().all(item))
}

fn binding(value: &Value) -> bool {
    fields(value, &[("@path", String)], &[])
}

fn function(value: &Value) -> bool {
    const FUNCTIONS: &[&str] = &[
        "@index",
        "required",
        "regex",
        "length",
        "numeric",
        "email",
        "formatString",
        "formatNumber",
        "formatCurrency",
        "formatDate",
        "pluralize",
        "openUrl",
        "and",
        "or",
        "not",
    ];
    if !fields(
        value,
        &[("@call", Choice(FUNCTIONS))],
        &[("args", Object), ("catalogId", Choice(&[BASIC_CATALOG_V1]))],
    ) {
        return false;
    }
    let args = &value["args"];
    match value["@call"].as_str() {
        Some("@index") => value.get("args").is_none() || fields(args, &[], &[("offset", Number)]),
        Some("and" | "or") => args["values"]
            .as_array()
            .is_some_and(|items| items.len() >= 2),
        Some("not") => args.get("value").is_some_and(|value| {
            value.is_boolean() || binding(value) || value.get("@call").is_some()
        }),
        _ => args.is_object(),
    }
}

/// Check only the changed nodes and their descendants. Every new cycle must
/// include a changed node, and an unresolved edge is valid during streaming.
pub(crate) fn component_graph(
    existing: &BTreeMap<std::string::String, Component>,
    pending: &BTreeMap<std::string::String, Component>,
    path: &str,
) -> Result<(), ProtocolError> {
    let mut active = HashMap::new();
    let mut stack = Vec::new();
    for id in pending.keys() {
        stack.push((id.as_str(), true));
        while let Some((id, entering)) = stack.pop() {
            if !entering {
                active.insert(id, false);
                continue;
            }
            match active.get(id) {
                Some(false) => continue,
                Some(true) => {
                    return Err(ProtocolError::at(
                        path,
                        format!("Component cycle at '{id}'."),
                    ));
                }
                None => {}
            }
            if let Some(node) = pending.get(id).or_else(|| existing.get(id)) {
                active.insert(id, true);
                stack.push((id, false));
                stack.extend(child_ids(node).map(|id| (id, true)));
            }
        }
    }
    Ok(())
}

fn child_ids(node: &Component) -> impl Iterator<Item = &str> {
    let children = node.property("children");
    children
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .chain(children["componentId"].as_str())
        .chain(
            ["child", "trigger", "content"]
                .into_iter()
                .filter_map(|key| node.property(key).as_str()),
        )
        .chain(
            node.property("tabs")
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|tab| tab["child"].as_str()),
        )
}
