use purlin::prelude::*;
use purlin_kit::Pagination;

use crate::domain::Page;

/// The kit validates `?limit=&offset=` generically; the domain re-checks them in its own
/// terms, so a repository can never be handed a window its own rules reject.
pub fn to_page(pagination: Pagination) -> Result<Page, HttpError> {
    Page::new(pagination.limit, pagination.offset).map_err(|error| HttpError::bad_request(error.to_string()))
}
