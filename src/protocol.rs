use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};

use crate::bindings::{self, Evaluator};
use crate::validation;

pub const BASIC_CATALOG_V1: &str =
    "https://a2ui.org/specification/v1_0/catalogs/basic/catalog.json";
pub const BASIC_CATALOG_V09: &str =
    "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json";
const BASIC_CATALOG_PATCH: &str =
    "https://a2ui.org/specification/v0_9_1/catalogs/basic/catalog.json";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Version {
    #[serde(rename = "1.0")]
    V1,
    #[serde(rename = "v0.9")]
    V09,
    #[serde(rename = "v0.9.1")]
    V091,
}

impl Version {
    pub(crate) fn supports(self, catalog: &str) -> bool {
        match self {
            Self::V1 => catalog == BASIC_CATALOG_V1,
            Self::V09 | Self::V091 => matches!(catalog, BASIC_CATALOG_V09 | BASIC_CATALOG_PATCH),
        }
    }

    pub(crate) fn path_key(self) -> &'static str {
        match self {
            Self::V1 => "@path",
            _ => "path",
        }
    }

    pub(crate) fn call_key(self) -> &'static str {
        match self {
            Self::V1 => "@call",
            _ => "call",
        }
    }

    pub(crate) fn checks_key(self) -> &'static str {
        match self {
            Self::V1 => "@checks",
            _ => "checks",
        }
    }
}

#[derive(Debug)]
pub struct ProtocolError {
    message: String,
    path: Option<String>,
}

impl ProtocolError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            path: None,
        }
    }

    pub(crate) fn at(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            path: Some(path.into()),
        }
    }

    /// The JSON Pointer within the inbound message, when known.
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(path) = &self.path {
            write!(formatter, "{path}: ")?;
        }
        self.message.fmt(formatter)
    }
}

impl std::error::Error for ProtocolError {}

impl From<serde_json::Error> for ProtocolError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(error.to_string())
    }
}

#[derive(Debug, Deserialize)]
pub struct Component {
    pub id: String,
    #[serde(rename = "component")]
    pub kind: String,
    #[serde(rename = "@catalogId")]
    pub catalog_id: Option<String>,
    #[serde(flatten)]
    pub properties: Map<String, Value>,
}

impl Component {
    pub fn property(&self, name: &str) -> &Value {
        self.properties.get(name).unwrap_or(&Value::Null)
    }
}

#[derive(Debug)]
pub struct Surface {
    pub id: String,
    pub version: Version,
    pub catalog_id: Option<String>,
    pub theme: Map<String, Value>,
    pub send_data_model: bool,
    pub(crate) generation: u64,
    components: BTreeMap<String, Component>,
    data_model: Value,
}

impl Surface {
    pub fn component(&self, id: &str) -> Option<&Component> {
        self.components.get(id)
    }

    pub fn data_model(&self) -> &Value {
        &self.data_model
    }

    /// Resolves a component property in the root or a template item's scope.
    pub fn resolve(&self, value: &Value, scope: &str) -> Value {
        Evaluator::new(&self.data_model, scope, self.version).resolve(value)
    }

    pub fn write_binding(
        &mut self,
        path: &str,
        scope: &str,
        value: Value,
    ) -> Result<(), ProtocolError> {
        bindings::update(
            &mut self.data_model,
            &bindings::absolute_path(scope, path),
            Some(value),
        )
    }

    pub(crate) fn supports_component(&self, component: &Component) -> bool {
        component
            .catalog_id
            .as_deref()
            .or(self.catalog_id.as_deref())
            .is_some_and(|catalog| self.version.supports(catalog))
    }

    pub(crate) fn failed_checks(&self, component: &Component, scope: &str) -> Vec<String> {
        component
            .property(self.version.checks_key())
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|check| {
                let condition = check.get("condition").unwrap_or(check);
                let result = self.resolve(condition, scope);
                if bindings::boolean(&result, self.version) {
                    return None;
                }
                if self.version == Version::V1
                    && let Some(message) = result["message"].as_str()
                {
                    return Some(message.to_owned());
                }
                Some(bindings::text(&self.resolve(&check["message"], scope)))
            })
            .collect()
    }

    pub fn action(&self, component_id: &str, scope: &str) -> Option<ClientEvent> {
        let component = self.component(component_id)?;
        let event = component.property("action").get("event")?;
        if !self.can_activate(component, scope) {
            return None;
        }
        let context = event["context"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(key, value)| (key.clone(), self.resolve(value, scope)))
            .collect();
        Some(ClientEvent {
            version: self.version,
            action: Action {
                name: event["name"].as_str()?.to_owned(),
                surface_id: self.id.clone(),
                source_component_id: component_id.to_owned(),
                timestamp: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                context,
                user_message: (self.version == Version::V1)
                    .then(|| self.resolve(&event["userMessage"], scope))
                    .and_then(|message| message.as_str().map(str::to_owned)),
            },
            data_model: self.send_data_model.then(|| self.data_model.clone()),
        })
    }

    pub(crate) fn can_activate(&self, component: &Component, scope: &str) -> bool {
        self.supports_component(component) && self.failed_checks(component, scope).is_empty()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ClientEvent {
    pub version: Version,
    pub action: Action,
    #[serde(skip)]
    data_model: Option<Value>,
}

impl ClientEvent {
    /// Metadata for the host's transport. The action itself is serialized separately.
    pub fn metadata(&self) -> Option<Value> {
        let data_model = self.data_model.as_ref()?;
        let surfaces = json!({ (self.action.surface_id.clone()): data_model });
        Some(match self.version {
            Version::V1 => json!({
                "a2uiRendererDataModel": { "version": self.version, "surfaces": surfaces }
            }),
            _ => json!({ "a2uiClientDataModel": { "surfaces": surfaces } }),
        })
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub name: String,
    pub surface_id: String,
    pub source_component_id: String,
    pub timestamp: String,
    pub context: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_message: Option<String>,
}

#[derive(Default, Debug)]
pub struct Document {
    surfaces: Vec<Surface>,
    next_generation: u64,
}

impl Document {
    pub fn surfaces(&self) -> &[Surface] {
        &self.surfaces
    }

    pub fn surface(&self, id: &str) -> Option<&Surface> {
        self.surfaces.iter().find(|surface| surface.id == id)
    }

    pub fn surface_mut(&mut self, id: &str) -> Option<&mut Surface> {
        self.surfaces.iter_mut().find(|surface| surface.id == id)
    }

    pub fn apply_json(&mut self, input: &str) -> Result<(), ProtocolError> {
        self.apply(serde_json::from_str(input)?)
    }

    pub fn apply(&mut self, mut envelope: Map<String, Value>) -> Result<(), ProtocolError> {
        if envelope.len() != 2 {
            return Err(ProtocolError::new(
                "Expected a version and exactly one A2UI message.",
            ));
        }
        let version: Version = decode(envelope.remove("version").unwrap_or(Value::Null))
            .map_err(|error| ProtocolError::at("/version", error.to_string()))?;
        let (kind, payload) = envelope
            .into_iter()
            .next()
            .expect("checked envelope length");
        if version == Version::V1 {
            validation::payload(&kind, &payload)?;
        }
        match kind.as_str() {
            "createSurface" => self.create(version, decode(payload)?),
            "updateComponents" => {
                let message: UpdateComponents = decode(payload)?;
                let surface = self.require_surface(&message.surface_id, version)?;
                let components = component_map(
                    message.components,
                    version,
                    surface.catalog_id.as_deref(),
                    "/updateComponents/components",
                )?;
                if version == Version::V1 {
                    validation::component_graph(
                        &surface.components,
                        &components,
                        "/updateComponents/components",
                    )?;
                }
                surface.components.extend(components);
                Ok(())
            }
            "updateDataModel" => {
                let message: UpdateData = decode(payload)?;
                let surface = self.require_surface(&message.surface_id, version)?;
                let value = message
                    .value
                    .filter(|value| version != Version::V1 || !value.is_null());
                bindings::update(
                    &mut surface.data_model,
                    message.path.as_deref().unwrap_or("/"),
                    value,
                )
                .map_err(|error| ProtocolError::at("/updateDataModel/path", error.to_string()))
            }
            "deleteSurface" => {
                let message: SurfaceRef = decode(payload)?;
                self.require_surface(&message.surface_id, version)?;
                self.surfaces
                    .retain(|surface| surface.id != message.surface_id);
                Ok(())
            }
            _ => Err(ProtocolError::new(format!(
                "Unsupported A2UI message: {kind}."
            ))),
        }
    }

    fn create(&mut self, version: Version, message: CreateSurface) -> Result<(), ProtocolError> {
        if message.surface_id.is_empty() {
            return Err(ProtocolError::new("Surface IDs cannot be empty."));
        }
        if self.surface(&message.surface_id).is_some() {
            return Err(ProtocolError::new(format!(
                "Surface '{}' already exists.",
                message.surface_id
            )));
        }
        match message.catalog_id.as_deref() {
            Some(catalog) if !version.supports(catalog) => {
                return Err(ProtocolError::new(format!(
                    "Unsupported catalog: {catalog}."
                )));
            }
            None if version != Version::V1 => {
                return Err(ProtocolError::new(
                    "This protocol version requires a catalogId.",
                ));
            }
            _ => {}
        }
        let components = component_map(
            message.components,
            version,
            message.catalog_id.as_deref(),
            "/createSurface/components",
        )?;
        if version == Version::V1 {
            validation::component_graph(
                &BTreeMap::new(),
                &components,
                "/createSurface/components",
            )?;
        }
        self.next_generation += 1;
        self.surfaces.push(Surface {
            id: message.surface_id,
            version,
            catalog_id: message.catalog_id,
            theme: message.theme,
            send_data_model: message.send_data_model,
            generation: self.next_generation,
            components,
            data_model: message.data_model,
        });
        Ok(())
    }

    fn require_surface(
        &mut self,
        id: &str,
        version: Version,
    ) -> Result<&mut Surface, ProtocolError> {
        let surface = self
            .surface_mut(id)
            .ok_or_else(|| ProtocolError::new(format!("Unknown surface '{id}'.")))?;
        if surface.version != version && (surface.version == Version::V1 || version == Version::V1)
        {
            return Err(ProtocolError::new(format!(
                "Version mismatch for surface '{id}'."
            )));
        }
        Ok(surface)
    }
}

fn decode<T: DeserializeOwned>(payload: Value) -> Result<T, ProtocolError> {
    Ok(serde_json::from_value(payload)?)
}

fn component_map(
    components: Vec<Component>,
    version: Version,
    catalog_id: Option<&str>,
    path: &str,
) -> Result<BTreeMap<String, Component>, ProtocolError> {
    let mut ids = BTreeSet::new();
    for (index, component) in components.iter().enumerate() {
        if version == Version::V1 {
            validation::component(component, catalog_id, &format!("{path}/{index}"))?;
        }
        if component.id.is_empty() || component.kind.is_empty() || !ids.insert(&component.id) {
            return Err(ProtocolError::at(
                format!("{path}/{index}/id"),
                "Component IDs and types must be nonempty, and each update must use distinct IDs.",
            ));
        }
    }
    Ok(components
        .into_iter()
        .map(|node| (node.id.clone(), node))
        .collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateSurface {
    surface_id: String,
    catalog_id: Option<String>,
    #[serde(default)]
    theme: Map<String, Value>,
    #[serde(default)]
    send_data_model: bool,
    #[serde(default)]
    components: Vec<Component>,
    #[serde(default = "empty_model")]
    data_model: Value,
}

fn empty_model() -> Value {
    Value::Object(Map::new())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SurfaceRef {
    surface_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateComponents {
    surface_id: String,
    components: Vec<Component>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateData {
    surface_id: String,
    path: Option<String>,
    #[serde(default, deserialize_with = "present_value")]
    value: Option<Value>,
}

fn present_value<'de, D: Deserializer<'de>>(source: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(source).map(Some)
}
