use nestrs::prelude::*;

use crate::error_log::ErrorLogFilter;
use crate::pagination::PaginationPipe;
use crate::request_context::RequestContext;
use crate::timing::TimingInterceptor;

/// Provides the kit's cross-cutting pieces. Import it in the root module (it
/// provides the error-logging filter [`Service`](crate::Service) registers) and in any
/// feature module whose controllers use the exports.
///
/// Exports [`TimingInterceptor`], [`PaginationPipe`] and the request-scoped
/// [`RequestContext`].
#[module(
    providers = [TimingInterceptor, PaginationPipe, RequestContext, ErrorLogFilter],
    exports = [TimingInterceptor, PaginationPipe, RequestContext],
)]
pub struct KitModule;
