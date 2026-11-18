use std::sync::Arc;

use purlin::http::axum::extract::Request;
use purlin::http::axum::middleware::Next;
use purlin::http::axum::response::Response;
use purlin::prelude::*;

#[injectable]
struct RequestContext {
    info: Arc<RequestInfo>,
}

#[injectable]
struct Trim;

impl Pipe for Trim {
    type Input = Query<std::collections::HashMap<String, String>>;
    type Output = usize;

    async fn transform(&self, Query(map): Self::Input) -> Result<usize, HttpError> {
        Ok(map.len())
    }
}

#[injectable]
struct Timing;

impl Interceptor for Timing {
    async fn intercept(&self, request: Request, next: Next) -> Response {
        next.run(request).await
    }
}

#[injectable]
struct Filter;

impl ExceptionFilter for Filter {
    async fn catch(&self, _error: &HttpError, _request: &RequestInfo) -> Option<Response> {
        None
    }
}

#[injectable]
struct C;

#[controller("/c")]
#[interceptor(Timing)]
impl C {
    #[get]
    async fn handler(&self, Inject(context): Inject<RequestContext>, Piped(count): Piped<Trim>) -> String {
        format!("{} {count}", context.info.uri)
    }
}

#[module(
    imports = [HttpModule::module()],
    providers = [Provider::of::<RequestContext>().request(), Trim, Timing, Filter],
    controllers = [C],
)]
struct AppModule;

fn main() {
    let _ = App::builder(AppModule).filter::<Filter>();
}
