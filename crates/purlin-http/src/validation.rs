use axum::Json;
use axum::extract::{FromRequest, Request};
use serde::de::DeserializeOwned;
use validator::Validate;

use crate::HttpError;

/// A JSON body that has been deserialised *and* validated.
///
/// Malformed JSON becomes a 400, rule violations a 422 with per-field details, and
/// the handler only ever sees a value that satisfies its `#[validate(..)]` rules.
///
/// ```ignore
/// #[derive(Deserialize, Validate)]
/// struct NewUser { #[validate(email)] email: String }
///
/// async fn create(&self, ValidatedJson(dto): ValidatedJson<NewUser>) { .. }
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct ValidatedJson<T>(pub T);

impl<S, T> FromRequest<S> for ValidatedJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Validate,
{
    type Rejection = HttpError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<T>::from_request(request, state)
            .await
            .map_err(|rejection| HttpError::bad_request(rejection.body_text()))?;
        value.validate()?;
        Ok(Self(value))
    }
}
