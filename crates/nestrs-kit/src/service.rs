use std::time::Duration;

use nestrs::http::Interceptor;

use nestrs::openapi::ApiInfo;
use nestrs::prelude::*;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use crate::error_log::ErrorLogFilter;

/// Builds the app-wide rate limit once the container exists.
type RateLimitFactory = Box<dyn FnOnce(&Container) -> Result<RateLimit, DiError> + Send>;

/// How a service starts: logging, API prefix, OpenAPI, global limits, serving.
///
/// It replaces the `main` every service writes the same way:
///
/// ```ignore
/// #[tokio::main]
/// async fn main() -> Result<(), AppError> {
///     Service::new("Blog API", env!("CARGO_PKG_VERSION"))
///         .description("Users and posts.")
///         .prefix("/api")
///         .timeout(Duration::from_secs(30))
///         .rate_limit(RateLimit::per_second(50).scope("global"))
///         .run(AppModule, |container| Ok(container.resolve::<AppConfig>()?.bind_addr.clone()))
///         .await
/// }
/// ```
///
/// What `run` does, in order: initialises logging (`RUST_LOG`, default
/// `info,tower_http=debug`); builds the app with [`ErrorLogFilter`] (so the root module
/// must import [`KitModule`](crate::KitModule)); applies the prefix; with
/// `--print-openapi`, prints the OpenAPI document to stdout and exits (logs go to
/// stderr); otherwise serves Swagger UI at `<prefix>/docs`, wraps the app in the
/// configured timeout and rate limit, request tracing, and listens until Ctrl-C / SIGTERM
/// with graceful shutdown.
pub struct Service {
    extra: Vec<Box<dyn FnOnce(App) -> App + Send>>,
    info: ApiInfo,
    prefix: String,
    timeout: Option<Duration>,
    rate_limit: Option<RateLimitFactory>,
    default_log_filter: String,
}

impl Service {
    /// A service called `title`, at `version`.
    pub fn new(title: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            extra: Vec::new(),
            info: ApiInfo::new(title, version),
            prefix: String::new(),
            timeout: None,
            rate_limit: None,
            default_log_filter: "info,tower_http=debug".to_owned(),
        }
    }

    /// A one-line description for the OpenAPI document.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.info = self.info.description(description);
        self
    }

    /// Mounts every route under `prefix` (`"/api"`).
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// A deadline for every request (504 when exceeded); see [`Timeout`].
    pub fn timeout(mut self, limit: Duration) -> Self {
        self.timeout = Some(limit);
        self
    }

    /// A rate limit around the whole app, applied before authentication; see [`RateLimit`].
    pub fn rate_limit(mut self, limit: RateLimit) -> Self {
        self.rate_limit = Some(Box::new(move |_| Ok(limit)));
        self
    }

    /// Like [`rate_limit`](Self::rate_limit), but built from the container, so the limit can use
    /// a provided store (Redis, to share one limit across instances):
    ///
    /// ```ignore
    /// .rate_limit_with(|c| Ok(RateLimit::per_second(50).scope("global").store(c.resolve::<dyn RateLimitStore>()?)))
    /// ```
    ///
    /// The provider must be visible from the root module (provided there, or exported by an
    /// imported module), as with exception filters.
    pub fn rate_limit_with(
        mut self,
        build: impl FnOnce(&Container) -> Result<RateLimit, DiError> + Send + 'static,
    ) -> Self {
        self.rate_limit = Some(Box::new(build));
        self
    }

    /// Wraps the whole app in any [`Interceptor`], such as one from `nestrs-interceptors`.
    ///
    /// Applied after the rate limit and the timeout, in the order called, and like
    /// `App::interceptor` each later one wraps the earlier ones: **the last call is
    /// outermost**. Put `RequestId` and `Metrics` last so they see everything.
    pub fn interceptor<I: Interceptor>(mut self, interceptor: I) -> Self {
        self.extra.push(Box::new(move |app| app.interceptor(interceptor)));
        self
    }

    /// The log filter used when `RUST_LOG` is not set.
    pub fn default_log_filter(mut self, filter: impl Into<String>) -> Self {
        self.default_log_filter = filter.into();
        self
    }

    /// Builds the app rooted at `root` and serves it. `bind` reads the listen address
    /// from the built container (usually from your config).
    pub async fn run<M: Module>(
        self,
        root: M,
        bind: impl FnOnce(&Container) -> Result<String, DiError>,
    ) -> Result<(), AppError> {
        let print_openapi = wants_openapi(std::env::args());
        self.init_logging(print_openapi);

        let app = App::builder(root).filter::<ErrorLogFilter>().build().await?;
        let bind_addr = bind(app.container())?;
        let rate_limit = self.rate_limit.map(|build| build(app.container())).transpose()?;
        let app = app.global_prefix(&self.prefix);

        if print_openapi {
            let document = serde_json::to_string_pretty(&app.openapi(&self.info)).expect("an OpenAPI document is JSON");
            println!("{document}");
            app.shutdown().await;
            return Ok(());
        }

        // Later interceptors wrap earlier ones: the deadline covers the limiter's work too.
        let mut app = app.serve_openapi(self.info);
        if let Some(limit) = rate_limit {
            app = app.interceptor(limit);
        }
        if let Some(limit) = self.timeout {
            app = app.interceptor(Timeout::new(limit));
        }
        for apply in self.extra {
            app = apply(app);
        }
        app.layer(TraceLayer::new_for_http()).listen(bind_addr.as_str()).await
    }

    fn init_logging(&self, to_stderr: bool) {
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&self.default_log_filter));
        let logs = tracing_subscriber::fmt().with_env_filter(filter);
        // stdout carries the OpenAPI document in `--print-openapi` mode, so logs must not.
        let _ = if to_stderr {
            logs.with_writer(std::io::stderr).try_init()
        } else {
            logs.try_init()
        };
    }
}

fn wants_openapi(args: impl IntoIterator<Item = String>) -> bool {
    args.into_iter().any(|argument| argument == "--print-openapi")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_openapi_flag_is_recognised_anywhere_in_the_arguments() {
        let args = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(wants_openapi(args(&["app", "--print-openapi"])));
        assert!(wants_openapi(args(&["app", "--x", "--print-openapi"])));
        assert!(!wants_openapi(args(&["app", "--print-openapi=1"])));
        assert!(!wants_openapi(args(&["app"])));
    }
}
