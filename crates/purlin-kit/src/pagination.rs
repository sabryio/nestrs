use purlin::openapi::IntoParams;
use purlin::openapi::utoipa::openapi::path::{Parameter, ParameterIn};
use purlin::prelude::*;
use serde::Deserialize;

/// A validated `?limit=&offset=` window.
///
/// Presentation-level and transport-agnostic: convert it into your own domain type
/// (a repository's `Page`) at the controller, so the domain never depends on this kit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pagination {
    /// Maximum number of items (1 to [`MAX_LIMIT`](Self::MAX_LIMIT)).
    pub limit: u64,
    /// Items to skip.
    pub offset: u64,
}

impl Pagination {
    /// The page size used when the caller does not say.
    pub const DEFAULT_LIMIT: u64 = 20;
    /// The largest page a caller may ask for.
    pub const MAX_LIMIT: u64 = 100;
}

impl Default for Pagination {
    fn default() -> Self {
        Self {
            limit: Self::DEFAULT_LIMIT,
            offset: 0,
        }
    }
}

/// The raw `?limit=&offset=` query string.
#[derive(Debug, Deserialize, IntoParams)]
pub struct PageQuery {
    /// Page size, 1 to 100 (default 20).
    limit: Option<u64>,
    /// Items to skip (default 0).
    offset: Option<u64>,
}

/// Pipe: turns `?limit=&offset=` into a validated [`Pagination`].
///
/// Use it as `Piped<PaginationPipe>`; out-of-range values are rejected with a 400
/// before the handler runs, and the two parameters appear in the OpenAPI document.
#[injectable]
pub struct PaginationPipe;

impl Pipe for PaginationPipe {
    type Input = Query<PageQuery>;
    type Output = Pagination;

    fn parameters() -> Vec<Parameter> {
        PageQuery::into_params(|| Some(ParameterIn::Query))
    }

    async fn transform(&self, Query(query): Query<PageQuery>) -> Result<Pagination, HttpError> {
        let limit = query.limit.unwrap_or(Pagination::DEFAULT_LIMIT);
        if !(1..=Pagination::MAX_LIMIT).contains(&limit) {
            return Err(HttpError::bad_request(format!(
                "limit must be between 1 and {}",
                Pagination::MAX_LIMIT
            )));
        }
        Ok(Pagination {
            limit,
            offset: query.offset.unwrap_or(0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn run(limit: Option<u64>, offset: Option<u64>) -> Result<Pagination, HttpError> {
        PaginationPipe.transform(Query(PageQuery { limit, offset })).await
    }

    #[tokio::test]
    async fn defaults_and_bounds() {
        assert_eq!(run(None, None).await.unwrap(), Pagination::default());
        assert_eq!(
            run(Some(100), Some(7)).await.unwrap(),
            Pagination { limit: 100, offset: 7 }
        );
        for bad in [0, 101] {
            let error = run(Some(bad), None).await.unwrap_err();
            assert_eq!(error.status().as_u16(), 400, "limit {bad}");
        }
    }

    #[test]
    fn documents_both_query_parameters() {
        let names: Vec<_> = PaginationPipe::parameters().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["limit", "offset"]);
    }
}
