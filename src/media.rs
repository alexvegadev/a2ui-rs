use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use a2ui_rs::ResourceKind;
use gpui_kit::http_client::{
    self, AsyncBody, HttpClient, Method, Request, Response, Url, anyhow, http::HeaderValue,
};
use url::Origin;

#[derive(Clone)]
pub struct MediaPolicy {
    image: Option<Origin>,
    navigation: Option<Origin>,
}

impl MediaPolicy {
    pub fn from_env() -> Result<Self, String> {
        Ok(Self {
            image: configured_origin("A2UI_IMAGE_ORIGIN")?,
            navigation: configured_origin("A2UI_NAVIGATION_ORIGIN")?,
        })
    }

    pub fn allows(&self, kind: ResourceKind, url: &Url) -> bool {
        let allowed = match kind {
            ResourceKind::Image => &self.image,
            ResourceKind::Navigation => &self.navigation,
        };
        url.username().is_empty()
            && url.password().is_none()
            && allowed
                .as_ref()
                .is_some_and(|origin| *origin == url.origin())
    }
}

fn configured_origin(name: &str) -> Result<Option<Origin>, String> {
    match std::env::var(name) {
        Ok(value) => {
            let invalid =
                || format!("{name} must be an HTTP(S) origin without credentials or a path.");
            let url = Url::parse(&value).map_err(|_| invalid())?;
            if !matches!(url.scheme(), "http" | "https")
                || !value.contains("://")
                || !url.has_host()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.path() != "/"
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(invalid());
            }
            Ok(Some(url.origin()))
        }
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(format!("{name}: {error}")),
    }
}

pub struct ImageHttpClient {
    agent: ureq::Agent,
    policy: MediaPolicy,
}

impl ImageHttpClient {
    pub fn new(policy: MediaPolicy) -> Self {
        Self {
            agent: ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(15)))
                .max_redirects(0)
                .proxy(None)
                .build()
                .into(),
            policy,
        }
    }
}

impl HttpClient for ImageHttpClient {
    fn user_agent(&self) -> Option<&HeaderValue> {
        None
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn send(
        &self,
        request: Request<AsyncBody>,
    ) -> Pin<Box<dyn Future<Output = http_client::Result<Response<AsyncBody>>> + Send>> {
        let agent = self.agent.clone();
        let policy = self.policy.clone();
        Box::pin(blocking::unblock(move || {
            let url = Url::parse(&request.uri().to_string())?;
            if request.method() != Method::GET || !policy.allows(ResourceKind::Image, &url) {
                return Err(anyhow!("Image request denied by the host policy."));
            }
            let mut response = agent.get(url.as_str()).call()?;
            if !response.status().is_success() {
                return Err(anyhow!(
                    "Image server did not return a successful response."
                ));
            }
            let mut out = Response::builder().status(response.status());
            if let Some(content_type) = response.headers().get("content-type") {
                out = out.header("content-type", content_type);
            }
            let bytes = response
                .body_mut()
                .with_config()
                .limit(8 * 1024 * 1024)
                .read_to_vec()?;
            Ok(out.body(AsyncBody::from(bytes))?)
        }))
    }
}
