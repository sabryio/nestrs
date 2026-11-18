//! Request scopes, pipes, interceptors and exception filters, through the macros.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use http_body_util::BodyExt;
use purlin::http::axum::body::Body;
use purlin::http::axum::extract::Request;
use purlin::http::axum::http::{HeaderValue, Method, Request as HttpRequest};
use purlin::http::axum::middleware::Next;
use purlin::http::axum::response::{IntoResponse, Response};
use purlin::prelude::*;
use serde::Deserialize;
use tower::ServiceExt;

static REQUEST_CONTEXTS_BUILT: AtomicUsize = AtomicUsize::new(0);

// ------------------------------------------------------------------ scopes

/// Request-scoped: one per request, and it knows which request that is.
#[injectable]
struct RequestContext {
    info: Arc<RequestInfo>,
    #[inject(default)]
    serial: Serial,
}

struct Serial(usize);
impl Default for Serial {
    fn default() -> Self {
        Self(REQUEST_CONTEXTS_BUILT.fetch_add(1, Ordering::SeqCst))
    }
}

/// Transient: a new one per resolution.
#[injectable]
struct Ticket {
    #[inject(default)]
    serial: Serial,
}

#[injectable]
struct ScopeController;

#[controller("/scope")]
impl ScopeController {
    #[get("/path")]
    async fn path(&self, Inject(context): Inject<RequestContext>) -> String {
        context.info.uri.path().to_owned()
    }

    /// Resolving the request-scoped provider twice yields the same instance;
    /// the transient one differs every time.
    #[get("/identity")]
    async fn identity(
        &self,
        Inject(first): Inject<RequestContext>,
        Inject(second): Inject<RequestContext>,
        Inject(ticket_a): Inject<Ticket>,
        Inject(ticket_b): Inject<Ticket>,
    ) -> String {
        format!(
            "{} {} {}",
            first.serial.0 == second.serial.0,
            ticket_a.serial.0 != ticket_b.serial.0,
            first.serial.0
        )
    }
}

#[module(
    imports = [HttpModule::module()],
    providers = [Provider::of::<RequestContext>().request(), Provider::of::<Ticket>().transient()],
    controllers = [ScopeController],
)]
struct ScopeModule;

async fn send(
    router: &purlin::http::axum::Router,
    method: Method,
    uri: &str,
    headers: &[(&str, &str)],
) -> (u16, String) {
    let mut builder = HttpRequest::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body = String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    (status, body)
}

#[tokio::test]
async fn request_scoped_providers_see_their_request() {
    let router = App::create(ScopeModule).await.unwrap().into_router();
    assert_eq!(send(&router, Method::GET, "/scope/path", &[]).await.1, "/scope/path");
}

#[tokio::test]
async fn request_scope_is_shared_within_a_request_and_not_across_requests() {
    let router = App::create(ScopeModule).await.unwrap().into_router();
    let (_, first) = send(&router, Method::GET, "/scope/identity", &[]).await;
    let (_, second) = send(&router, Method::GET, "/scope/identity", &[]).await;
    assert!(first.starts_with("true true"), "{first}");
    assert!(second.starts_with("true true"), "{second}");
    assert_ne!(first, second, "each request builds its own RequestContext");
}

#[injectable]
struct UsesInjectMissing;

#[controller("/missing")]
impl UsesInjectMissing {
    #[get]
    async fn handler(&self, Inject(_): Inject<RequestContext>) {}
}

#[module(controllers = [UsesInjectMissing])]
struct MissingScopedProvider;

#[tokio::test]
async fn inject_parameters_are_checked_at_startup() {
    let error = App::create(MissingScopedProvider).await.unwrap_err();
    assert!(
        matches!(error, AppError::Di(DiError::MissingProvider { .. })),
        "a handler asking for an unprovided type must fail at startup: {error}"
    );
}

#[injectable]
struct Capturing {
    _context: Arc<RequestContext>,
}

#[module(imports = [HttpModule::module()], providers = [Provider::of::<RequestContext>().request(), Capturing])]
struct CapturingModule;

#[tokio::test]
async fn singletons_cannot_capture_request_scoped_providers() {
    let error = App::create(CapturingModule).await.unwrap_err();
    assert!(matches!(error, AppError::Di(DiError::ScopeMismatch { .. })), "{error}");
}

// ------------------------------------------------------------------- pipes

#[derive(Deserialize)]
struct RawPage {
    limit: Option<u32>,
}

#[derive(Debug)]
struct Limit(u32);

/// Injectable pipe: clamps `?limit=` into 1..=max, rejecting zero.
#[injectable]
struct LimitPipe {
    #[inject(default)]
    max: Max,
}
struct Max(u32);
impl Default for Max {
    fn default() -> Self {
        Self(50)
    }
}

impl Pipe for LimitPipe {
    type Input = Query<RawPage>;
    type Output = Limit;

    async fn transform(&self, Query(raw): Query<RawPage>) -> Result<Limit, HttpError> {
        match raw.limit.unwrap_or(10) {
            0 => Err(HttpError::bad_request("limit must be at least 1")),
            n => Ok(Limit(n.min(self.max.0))),
        }
    }
}

#[injectable]
struct PipeController;

#[controller("/pipe")]
impl PipeController {
    #[get]
    async fn limit(&self, Piped(Limit(limit)): Piped<LimitPipe>) -> String {
        limit.to_string()
    }
}

#[module(imports = [HttpModule::module()], providers = [LimitPipe], controllers = [PipeController])]
struct PipeModule;

#[tokio::test]
async fn pipes_transform_and_reject_inputs() {
    let router = App::create(PipeModule).await.unwrap().into_router();
    assert_eq!(send(&router, Method::GET, "/pipe", &[]).await, (200, "10".into()));
    assert_eq!(
        send(&router, Method::GET, "/pipe?limit=500", &[]).await,
        (200, "50".into())
    );
    let (status, body) = send(&router, Method::GET, "/pipe?limit=0", &[]).await;
    assert_eq!(status, 400);
    assert!(body.contains("at least 1"), "{body}");
    // The input extractor's own rejection passes through untouched.
    assert_eq!(send(&router, Method::GET, "/pipe?limit=abc", &[]).await.0, 400);
}

// ------------------------------------------------------------ interceptors

#[derive(Default)]
#[injectable]
struct Trace {
    #[inject(default)]
    events: Mutex<Vec<String>>,
}

#[injectable]
struct GuardWithTrace {
    trace: Arc<Trace>,
}
impl Guard for GuardWithTrace {
    async fn check(&self, _parts: &mut Parts) -> Result<(), HttpError> {
        self.trace.events.lock().unwrap().push("guard".to_owned());
        Ok(())
    }
}

#[injectable]
struct Outer {
    trace: Arc<Trace>,
}
#[injectable]
struct Inner {
    trace: Arc<Trace>,
}

impl Interceptor for Outer {
    async fn intercept(&self, request: Request, next: Next) -> Response {
        self.trace.events.lock().unwrap().push("outer before".to_owned());
        let mut response = next.run(request).await;
        self.trace.events.lock().unwrap().push("outer after".to_owned());
        response.headers_mut().insert("x-outer", HeaderValue::from_static("1"));
        response
    }
}
impl Interceptor for Inner {
    async fn intercept(&self, request: Request, next: Next) -> Response {
        self.trace.events.lock().unwrap().push("inner before".to_owned());
        let response = next.run(request).await;
        self.trace.events.lock().unwrap().push("inner after".to_owned());
        response
    }
}

#[injectable]
struct InterceptedController {
    trace: Arc<Trace>,
}

#[controller("/intercepted")]
impl InterceptedController {
    #[get]
    #[interceptor(Outer)]
    #[interceptor(Inner)]
    #[guard(GuardWithTrace)]
    async fn handler(&self) -> &'static str {
        self.trace.events.lock().unwrap().push("handler".to_owned());
        "done"
    }
}

#[module(providers = [Trace, GuardWithTrace, Outer, Inner], controllers = [InterceptedController])]
struct InterceptorModule;

#[tokio::test]
async fn guards_run_first_then_interceptors_outermost_first_around_the_handler() {
    let app = App::create(InterceptorModule).await.unwrap();
    let trace = app.container().resolve::<Trace>().unwrap();
    let router = app.into_router();

    let request = HttpRequest::builder().uri("/intercepted").body(Body::empty()).unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.headers()["x-outer"], "1");
    assert_eq!(
        *trace.events.lock().unwrap(),
        [
            "guard",
            "outer before",
            "inner before",
            "handler",
            "inner after",
            "outer after"
        ]
    );
}

// ------------------------------------------- inline interceptors, limits

#[injectable]
struct Limited;

/// A class-level limit is one allowance for all the controller's routes; a route-level
/// one is its own. Neither needs to be registered as a provider.
#[controller("/limited")]
#[interceptor(RateLimit::per_minute(3).by(KeyBy::Global))]
impl Limited {
    #[get("/a")]
    async fn a(&self) -> &'static str {
        "a"
    }
    #[get("/b")]
    async fn b(&self) -> &'static str {
        "b"
    }
    #[get("/strict")]
    #[interceptor(RateLimit::per_minute(1).by(KeyBy::Global))]
    async fn strict(&self) -> &'static str {
        "strict"
    }
    #[get("/slow")]
    #[interceptor(Timeout::millis(20))]
    async fn slow(&self) -> &'static str {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        "late"
    }
}

#[module(controllers = [Limited])]
struct LimitedModule;

#[tokio::test]
async fn inline_interceptors_limit_rates_and_deadlines_without_registration() {
    let router = App::create(LimitedModule).await.unwrap().into_router();
    let get = |path: &'static str| {
        let router = router.clone();
        async move {
            router
                .oneshot(HttpRequest::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap()
        }
    };

    // /a and /b share the class-level allowance of 3 (the strict route counts too, as
    // it sits under the class interceptor as well).
    assert_eq!(get("/limited/a").await.status(), 200);
    assert_eq!(get("/limited/b").await.status(), 200);
    assert_eq!(get("/limited/strict").await.status(), 200);
    let limited = get("/limited/a").await;
    assert_eq!(limited.status(), 429);
    assert!(limited.headers().contains_key("retry-after"));

    // The route-level limit is separate and already spent.
    let slow = get("/limited/slow").await;
    assert_eq!(
        slow.status(),
        429,
        "the class-level allowance is gone, so even /slow is refused"
    );
}

// ------------------------------------------------------------------ filters

#[derive(Default)]
#[injectable]
struct Seen {
    #[inject(default)]
    errors: Mutex<Vec<(u16, String)>>,
}

/// Observes every error without changing the response.
#[injectable]
struct Observer {
    seen: Arc<Seen>,
}
impl ExceptionFilter for Observer {
    async fn catch(&self, error: &HttpError, request: &RequestInfo) -> Option<Response> {
        self.seen
            .errors
            .lock()
            .unwrap()
            .push((error.status().as_u16(), request.uri.path().to_owned()));
        None
    }
}

/// Rewrites 404s into plain text.
#[injectable]
struct PlainNotFound;
impl ExceptionFilter for PlainNotFound {
    async fn catch(&self, error: &HttpError, _request: &RequestInfo) -> Option<Response> {
        matches!(error, HttpError::NotFound(_)).then(|| (error.status(), "nothing here").into_response())
    }
}

#[injectable]
struct FailingController;

#[controller("/fail")]
impl FailingController {
    #[get("/conflict")]
    async fn conflict(&self) -> Result<(), HttpError> {
        Err(HttpError::conflict("taken"))
    }
}

#[module(imports = [HttpModule::module()], providers = [Seen, Observer, PlainNotFound], controllers = [FailingController])]
struct FilterModule;

#[tokio::test]
async fn exception_filters_observe_and_replace_error_responses() {
    let app = App::builder(FilterModule)
        .filter::<Observer>()
        .filter::<PlainNotFound>()
        .build()
        .await
        .unwrap();
    let seen = app.container().resolve::<Seen>().unwrap();
    let router = app.into_router();

    let (status, body) = send(&router, Method::GET, "/fail/conflict", &[]).await;
    assert_eq!(status, 409);
    assert!(body.contains("\"code\":\"conflict\""), "untouched: {body}");

    let (status, body) = send(&router, Method::GET, "/nowhere", &[]).await;
    assert_eq!((status, body.as_str()), (404, "nothing here"));

    assert_eq!(
        *seen.errors.lock().unwrap(),
        [(409, "/fail/conflict".to_owned()), (404, "/nowhere".to_owned())]
    );
}

#[tokio::test]
async fn a_filter_nobody_provides_fails_the_build() {
    let error = App::builder(ScopeModule)
        .filter::<Observer>()
        .build()
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::Di(DiError::NotFound { .. })), "{error}");
}

// ------------------------------------------------ roles and permissions

/// Authenticates from `x-user: <subject>[; roles=a,b][; perms=x,y]`.
#[injectable]
struct HeaderAuth;
impl Guard for HeaderAuth {
    async fn check(&self, parts: &mut Parts) -> Result<(), HttpError> {
        let Some(header) = parts.headers.get("x-user").and_then(|v| v.to_str().ok()) else {
            return Err(HttpError::unauthorized("who are you?"));
        };
        let mut fields = header.split(';').map(str::trim);
        let mut principal = Principal::new(fields.next().unwrap_or_default());
        for field in fields {
            if let Some(roles) = field.strip_prefix("roles=") {
                principal = principal.with_roles(roles.split(','));
            } else if let Some(perms) = field.strip_prefix("perms=") {
                principal = principal.with_permissions(perms.split(','));
            }
        }
        parts.extensions.insert(principal);
        Ok(())
    }
}

#[injectable]
struct Vault;

/// The class demands the `staff` role of everyone; routes add to that.
#[controller("/vault")]
#[guard(HeaderAuth)]
#[roles("staff")]
impl Vault {
    #[get("/lobby")]
    async fn lobby(&self) -> &'static str {
        "lobby"
    }
    #[get("/safe")]
    #[roles("manager", "owner")]
    async fn safe(&self) -> &'static str {
        "safe"
    }
    #[delete("/records")]
    #[permissions("records:delete", "audit:write")]
    async fn shred(&self) -> &'static str {
        "shredded"
    }
    #[get("/me")]
    async fn me(&self, principal: Principal) -> String {
        principal.subject().to_owned()
    }
}

#[module(providers = [HeaderAuth], controllers = [Vault])]
struct VaultModule;

async fn vault(method: &str, path: &str, user: Option<&str>) -> (StatusCode, serde_json::Value) {
    let router = App::create(VaultModule).await.unwrap().into_router();
    let mut request = HttpRequest::builder().method(method).uri(path);
    if let Some(user) = user {
        request = request.header("x-user", user);
    }
    let response = router.oneshot(request.body(Body::empty()).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

#[tokio::test]
async fn requirements_answer_401_without_a_principal_and_403_without_privileges() {
    let (status, _) = vault("GET", "/vault/lobby", None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the guard rejects before any requirement"
    );

    let (status, body) = vault("GET", "/vault/lobby", Some("ann")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "authenticated, but not staff");
    assert_eq!(body["error"]["code"], "forbidden");
    assert!(
        !body.to_string().contains("staff"),
        "the response does not reveal what was required"
    );

    let (status, _) = vault("GET", "/vault/lobby", Some("ann; roles=staff")).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn class_and_route_requirements_must_both_hold() {
    // staff (class) + one of manager/owner (route)
    assert_eq!(
        vault("GET", "/vault/safe", Some("a; roles=staff")).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        vault("GET", "/vault/safe", Some("a; roles=manager")).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        vault("GET", "/vault/safe", Some("a; roles=staff,owner")).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn permissions_need_all_of_them_and_honour_wildcards() {
    let staff = "a; roles=staff";
    assert_eq!(
        vault(
            "DELETE",
            "/vault/records",
            Some(&format!("{staff}; perms=records:delete"))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        vault(
            "DELETE",
            "/vault/records",
            Some(&format!("{staff}; perms=records:delete,audit:write"))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        vault("DELETE", "/vault/records", Some(&format!("{staff}; perms=*")))
            .await
            .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn handlers_can_take_the_principal() {
    let (status, body) = vault("GET", "/vault/me", Some("zoe; roles=staff")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::Value::Null, "plain text body, not JSON");
}

// ------------------------------------------------ bearer authentication

use purlin::http::{BearerAuth, StaticTokens};

#[injectable]
struct Club;

#[controller("/club")]
#[guard(BearerAuth)]
impl Club {
    #[get("/members")]
    #[roles("member")]
    async fn members(&self, principal: Principal) -> String {
        format!("hello {}", principal.subject())
    }
}

#[module(
    providers = [
        StaticTokens::new().token("t-ann", Principal::new("ann").with_roles(["member"])).token("t-bob", Principal::new("bob")).provider(),
        BearerAuth,
    ],
    controllers = [Club],
)]
struct ClubModule;

#[tokio::test]
async fn bearer_auth_authenticates_then_roles_authorize() {
    let router = App::create(ClubModule).await.unwrap().into_router();
    let call = |token: Option<&'static str>| {
        let router = router.clone();
        async move {
            let mut request = HttpRequest::builder().uri("/club/members");
            if let Some(token) = token {
                request = request.header("authorization", format!("Bearer {token}"));
            }
            router
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap()
                .status()
        }
    };
    assert_eq!(call(None).await, StatusCode::UNAUTHORIZED);
    assert_eq!(call(Some("nope")).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        call(Some("t-bob")).await,
        StatusCode::FORBIDDEN,
        "authenticated but no member role"
    );
    assert_eq!(call(Some("t-ann")).await, StatusCode::OK);
}

// ------------------------------------------------ optional authentication

use purlin::http::OptionalBearerAuth;

#[injectable]
struct Lobby;

#[controller("/lobby")]
impl Lobby {
    #[get("/hello")]
    #[guard(OptionalBearerAuth)]
    async fn hello(&self, who: Option<Principal>) -> String {
        match who {
            Some(principal) => format!("hello {}", principal.subject()),
            None => "hello stranger".to_owned(),
        }
    }
}

#[module(
    providers = [StaticTokens::new().token("t-ann", Principal::new("ann")).provider(), OptionalBearerAuth],
    controllers = [Lobby],
)]
struct LobbyModule;

#[tokio::test]
async fn optional_auth_serves_strangers_and_members_but_rejects_bad_tokens() {
    let router = App::create(LobbyModule).await.unwrap().into_router();
    let call = |token: Option<&'static str>| {
        let router = router.clone();
        async move {
            let mut request = HttpRequest::builder().uri("/lobby/hello");
            if let Some(token) = token {
                request = request.header("authorization", format!("Bearer {token}"));
            }
            let response = router.oneshot(request.body(Body::empty()).unwrap()).await.unwrap();
            let status = response.status();
            let body = http_body_util::BodyExt::collect(response.into_body())
                .await
                .unwrap()
                .to_bytes();
            (status, String::from_utf8(body.to_vec()).unwrap())
        }
    };
    assert_eq!(call(None).await, (StatusCode::OK, "hello stranger".to_owned()));
    assert_eq!(call(Some("t-ann")).await, (StatusCode::OK, "hello ann".to_owned()));
    assert_eq!(call(Some("stale")).await.0, StatusCode::UNAUTHORIZED);
}
