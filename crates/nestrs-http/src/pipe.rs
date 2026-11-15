use std::future::Future;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};

use crate::HttpError;
use crate::scope::scope_of;

/// Turns one extracted value into another, or rejects it: parsing, normalising,
/// validating, loading an entity from an id.
///
/// Pipes are injectable providers, so they can use services. Declare the extractor
/// they read from as [`Input`](Self::Input) and use them as `Piped<MyPipe>` in a
/// handler:
///
/// ```ignore
/// struct PageParams;
/// impl Pipe for PageParams {
///     type Input = Query<RawPage>;
///     type Output = Page;
///     async fn transform(&self, Query(raw): Query<RawPage>) -> Result<Page, HttpError> { .. }
/// }
///
/// #[get]
/// async fn list(&self, Piped(page): Piped<PageParams>) -> .. { .. }
/// ```
pub trait Pipe: Send + Sync + 'static {
    /// The extractor the pipe reads its input with.
    type Input: Send;
    /// What the handler receives.
    type Output: Send;

    /// Query parameters this pipe reads, for the generated OpenAPI document.
    /// Optional: the default documents none.
    fn parameters() -> Vec<utoipa::openapi::path::Parameter> {
        Vec::new()
    }

    /// Transforms or rejects the input.
    fn transform(&self, input: Self::Input) -> impl Future<Output = Result<Self::Output, HttpError>> + Send;
}

/// Extractor that runs the pipe `P`: extracts `P::Input`, resolves `P` from the
/// request scope, and hands the handler `P::Output`.
#[derive(Debug)]
pub struct Piped<P: Pipe>(pub P::Output);

impl<S, P> FromRequestParts<S> for Piped<P>
where
    S: Send + Sync,
    P: Pipe,
    P::Input: FromRequestParts<S>,
    <P::Input as FromRequestParts<S>>::Rejection: IntoResponse,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let input = <P::Input as FromRequestParts<S>>::from_request_parts(parts, state)
            .await
            .map_err(IntoResponse::into_response)?;
        let pipe = scope_of(parts)
            .map_err(IntoResponse::into_response)?
            .resolve::<P>()
            .await
            .map_err(|error| HttpError::internal(error.to_string()).into_response())?;
        pipe.transform(input)
            .await
            .map(Self)
            .map_err(IntoResponse::into_response)
    }
}
