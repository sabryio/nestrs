use nestrs::prelude::*;

use crate::application::ApplicationError;

/// The single place where application failures meet HTTP status codes.
impl From<ApplicationError> for HttpError {
    fn from(error: ApplicationError) -> Self {
        match error {
            ApplicationError::Invalid(_) => Self::bad_request(error.to_string()),
            ApplicationError::UserNotFound(_) | ApplicationError::PostNotFound(_) => Self::not_found(error.to_string()),
            ApplicationError::AuthorNotFound(_) => Self::bad_request(error.to_string()),
            ApplicationError::EmailTaken(_) => Self::conflict(error.to_string()),
            // Storage details are logged by `HttpError`, never sent to the client.
            ApplicationError::Repository(_) => Self::internal(error.to_string()),
        }
    }
}
