use std::sync::Arc;

use nestrs::prelude::*;
use uuid::Uuid;

/// Per-request facts, built once per request (a *request-scoped* provider).
///
/// Handlers receive it with `Inject<RequestContext>`. Nothing about the request is
/// shared between requests, which a singleton could not guarantee.
#[injectable(scope = request)]
pub struct RequestContext {
    info: Arc<RequestInfo>,
    #[inject(default)]
    generated: GeneratedId,
}

/// An id minted for requests that arrive without `x-request-id`.
pub struct GeneratedId(Uuid);

impl Default for GeneratedId {
    fn default() -> Self {
        Self(Uuid::new_v4())
    }
}

impl RequestContext {
    /// The caller's `x-request-id`, or an id generated for this request.
    pub fn request_id(&self) -> String {
        self.info
            .headers
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .map_or_else(|| self.generated.0.to_string(), str::to_owned)
    }
}
