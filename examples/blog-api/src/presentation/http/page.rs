use nestrs::prelude::*;
use nestrs_kit::Pagination;

use crate::domain::Page;

impl TryFrom<Pagination> for Page {
    type Error = HttpError;

    /// The kit validates `?limit=&offset=` generically; the domain re-checks them in its own
    /// terms, so a repository can never be handed a window its own rules reject.
    fn try_from(pagination: Pagination) -> Result<Self, Self::Error> {
        Page::new(pagination.limit, pagination.offset).map_err(|error| HttpError::bad_request(error.to_string()))
    }
}
