//! A native A2UI client for embedding agent-provided surfaces in GPUI applications.

mod bindings;
mod protocol;
mod validation;
mod view;

pub use protocol::{
    Action, BASIC_CATALOG_V1, BASIC_CATALOG_V09, ClientEvent, Component, Document, ProtocolError,
    Surface, Version,
};
pub use view::{A2uiView, ResourceKind};
