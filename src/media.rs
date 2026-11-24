use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use gpui_kit::http_client::{
    self, AsyncBody, HttpClient, Method, Request, Response, Url, anyhow, http::HeaderValue,
};

pub struct ImageHttpClient {
    agent: ureq::Agent,
}

impl ImageHttpClient {
    pub fn new() -> Self {
        Self {
            agent: ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(15)))
                .max_redirects(4)
                .build()
                .into(),
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
        Box::pin(blocking::unblock(move || {
            if request.method() != Method::GET
                || !matches!(request.uri().scheme_str(), Some("http" | "https"))
            {
                return Err(anyhow!("Image requests require an HTTP(S) GET."));
            }
            let mut response = agent.get(request.uri().to_string()).call()?;
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
