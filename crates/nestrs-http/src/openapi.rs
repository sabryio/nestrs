//! OpenAPI 3 generation from `#[controller]` declarations.
//!
//! The macro records, per route: method, path, handler docs (`///` becomes summary
//! and description), path parameters, `Query<T>` parameters, the `Json<T>` /
//! `ValidatedJson<T>` request body and the response body. Types describe themselves
//! with [`utoipa`] (`#[derive(ToSchema)]`, `#[derive(IntoParams)]`). A type that does
//! not implement those traits still works; it is simply documented without a schema.
//!
//! Serve the result with `App::serve_openapi`, or get the document with
//! `App::openapi`.

use std::collections::BTreeMap;
use std::marker::PhantomData;

use utoipa::openapi::path::{
    HttpMethod, Operation, OperationBuilder, Parameter, ParameterBuilder, ParameterIn, PathItem,
};
use utoipa::openapi::request_body::RequestBodyBuilder;
use utoipa::openapi::response::ResponseBuilder;
use utoipa::openapi::schema::{
    ArrayBuilder, ComponentsBuilder, KnownFormat, Object, ObjectBuilder, OneOfBuilder, Schema, SchemaFormat,
    SchemaType, Type,
};
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityRequirement, SecurityScheme};
use utoipa::openapi::{ContentBuilder, InfoBuilder, OpenApi, OpenApiBuilder, PathsBuilder, Ref, RefOr, Required};
use utoipa::{IntoParams, PartialSchema, ToSchema};

/// Title, version and description of the generated document.
#[derive(Debug, Clone)]
pub struct ApiInfo {
    /// API title.
    pub title: String,
    /// API version string.
    pub version: String,
    /// Optional longer description.
    pub description: Option<String>,
}

impl ApiInfo {
    /// Info with a title and version.
    pub fn new(title: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            version: version.into(),
            description: None,
        }
    }

    /// Adds a description.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// A documented body: the schema's name plus every schema it needs.
#[derive(Clone)]
pub struct BodyDoc {
    name: String,
    schemas: Vec<(String, RefOr<Schema>)>,
    array: bool,
}

impl BodyDoc {
    /// Documents a JSON body of type `T`, for routes that have no `#[get]`/`#[post]` to read it from.
    pub fn of<T: ToSchema>() -> Self {
        let mut schemas = vec![(T::name().into_owned(), T::schema())];
        T::schemas(&mut schemas);
        Self {
            name: T::name().into_owned(),
            schemas,
            array: false,
        }
    }

    /// Documents the body as an array of this schema (`Vec<T>` bodies).
    pub fn into_array(mut self) -> Self {
        self.array = true;
        self
    }
}

/// The schema of a `uuid::Uuid` path parameter: a string with format `uuid`.
pub fn uuid_schema() -> RefOr<Schema> {
    RefOr::T(Schema::Object(
        ObjectBuilder::new()
            .schema_type(SchemaType::Type(Type::String))
            .format(Some(SchemaFormat::KnownFormat(KnownFormat::Uuid)))
            .build(),
    ))
}

/// One response of a route.
#[derive(Clone)]
pub struct ResponseDoc {
    status: u16,
    body: Option<BodyDoc>,
    /// The body is a `text/event-stream` whose messages are `body`.
    events: bool,
}

impl ResponseDoc {
    /// A response with `status` and an optional JSON body.
    pub fn new(status: u16, body: Option<BodyDoc>) -> Self {
        Self {
            status,
            body,
            events: false,
        }
    }

    /// A `text/event-stream` response whose messages are `body` (see `EventStream`).
    pub fn event_stream(status: u16, body: Option<BodyDoc>) -> Self {
        Self {
            status,
            body,
            events: true,
        }
    }
}

/// Everything `#[controller]` knows about one route. Built by generated code.
#[derive(Clone)]
pub struct RouteDoc {
    method: &'static str,
    path: String,
    handler: &'static str,
    controller: &'static str,
    /// First line of the handler's doc comment.
    pub summary: Option<String>,
    /// The rest of the handler's doc comment.
    pub description: Option<String>,
    path_params: Vec<(String, RefOr<Schema>)>,
    query: Vec<Parameter>,
    body: Option<BodyDoc>,
    responses: Vec<ResponseDoc>,
    /// Extra error responses declared with `#[response(status, "description")]`.
    declared_errors: Vec<(u16, String)>,
    secured: bool,
    anonymous_ok: bool,
    /// What `#[roles]` / `#[permissions]` demand, in words.
    requirements: Vec<String>,
    validated: bool,
}

impl RouteDoc {
    /// Starts a route doc.
    pub fn new(method: &'static str, path: String, handler: &'static str, controller: &'static str) -> Self {
        Self {
            method,
            path,
            handler,
            controller,
            summary: None,
            description: None,
            path_params: Vec::new(),
            query: Vec::new(),
            body: None,
            responses: Vec::new(),
            declared_errors: Vec::new(),
            secured: false,
            anonymous_ok: false,
            requirements: Vec::new(),
            validated: false,
        }
    }

    /// A route documented by hand (see `Mount`): `operation` becomes the operation id.
    pub fn route(method: &'static str, path: impl Into<String>, operation: &'static str) -> Self {
        Self::new(method, path.into(), operation, "Other").response(ResponseDoc::new(200, None))
    }

    /// `GET path`.
    pub fn get(path: impl Into<String>, operation: &'static str) -> Self {
        Self::route("get", path, operation)
    }

    /// `POST path`.
    pub fn post(path: impl Into<String>, operation: &'static str) -> Self {
        Self::route("post", path, operation)
    }

    /// `PUT path`.
    pub fn put(path: impl Into<String>, operation: &'static str) -> Self {
        Self::route("put", path, operation)
    }

    /// `DELETE path`.
    pub fn delete(path: impl Into<String>, operation: &'static str) -> Self {
        Self::route("delete", path, operation)
    }

    /// Groups the route under `tag` in the OpenAPI output (controllers use their name).
    pub fn tagged(mut self, tag: &'static str) -> Self {
        self.controller = tag;
        self
    }

    /// Documents the handler's doc comment: first line is the summary.
    pub fn docs(mut self, lines: &[&str]) -> Self {
        let text: Vec<&str> = lines.iter().map(|line| line.trim()).collect();
        let joined = text.join("\n");
        let joined = joined.trim();
        let (summary, description) = match joined.split_once("\n\n") {
            Some((summary, rest)) => (summary, rest.trim()),
            None => (joined, ""),
        };
        self.summary = (!summary.is_empty()).then(|| summary.replace('\n', " "));
        self.description = (!description.is_empty()).then(|| description.to_owned());
        self
    }

    /// Adds a path parameter and its schema.
    pub fn path_param(mut self, name: &str, schema: RefOr<Schema>) -> Self {
        self.path_params.push((name.to_owned(), schema));
        self
    }

    /// Adds `Query<T>` parameters.
    pub fn query(mut self, mut params: Vec<Parameter>) -> Self {
        self.query.append(&mut params);
        self
    }

    /// Sets the request body.
    pub fn request_body(mut self, body: Option<BodyDoc>) -> Self {
        self.body = body;
        self
    }

    /// Adds a response.
    pub fn response(mut self, response: ResponseDoc) -> Self {
        self.responses.push(response);
        self
    }

    /// Documents an extra error response (`#[response(404, "Post not found")]`). It
    /// replaces the generic description the generator would otherwise add for that status.
    pub fn declared_error(mut self, status: u16, description: &str) -> Self {
        self.declared_errors.push((status, description.to_owned()));
        self
    }

    /// Marks a secured route as also open to callers with no token (every guard on it is
    /// optional). Ignored when the route has `#[roles]` / `#[permissions]`, which need a caller.
    pub fn anonymous_ok(mut self, anonymous_ok: bool) -> Self {
        self.anonymous_ok = anonymous_ok;
        self
    }

    /// Marks the route as requiring a bearer token (it has a guard).
    pub fn secured(mut self, secured: bool) -> Self {
        self.secured = secured;
        self
    }

    /// Documents what the route demands of the caller (`"role admin"`). A route with
    /// requirements is secured, and gets a 403 response.
    pub fn requires(mut self, requirement: impl Into<String>) -> Self {
        self.requirements.push(requirement.into());
        self
    }

    /// Marks the route as validating its body (adds a 422 response).
    pub fn validated(mut self, validated: bool) -> Self {
        self.validated = validated;
        self
    }
}

// ----------------------------------------------------------------- probing
//
// `#[controller]` must work for types that implement `ToSchema` and for those that
// do not. These pairs use autoref specialisation: calling `(&Probe::<T>::new()).m()`
// picks the `Via*` impl when the bound holds, and the `Without*` fallback otherwise.

/// Type-level probe used by generated code.
pub struct Probe<T>(PhantomData<T>);

impl<T> Probe<T> {
    /// Creates the probe.
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

impl<T> Default for Probe<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// `T: ToSchema`: the body is documented.
pub trait ViaToSchema {
    /// Documents `T` as a JSON body.
    fn schema_body(&self) -> Option<BodyDoc>;
}
impl<T: ToSchema> ViaToSchema for Probe<T> {
    fn schema_body(&self) -> Option<BodyDoc> {
        let mut schemas = Vec::new();
        schemas.push((T::name().into_owned(), T::schema()));
        T::schemas(&mut schemas);
        Some(BodyDoc {
            name: T::name().into_owned(),
            schemas,
            array: false,
        })
    }
}
/// `G: Guard`: reports whether the guard lets anonymous callers through.
pub trait ViaGuard {
    /// `G::ANONYMOUS_OK`.
    fn guard_anonymous_ok(&self) -> bool;
}
impl<G: crate::Guard> ViaGuard for Probe<G> {
    fn guard_anonymous_ok(&self) -> bool {
        G::ANONYMOUS_OK
    }
}
/// Fallback for a type that is not a guard. The route's own `guarded(..)` call reports that
/// error once, on the guard expression; this keeps the docs from reporting it a second time.
pub trait WithoutGuard {
    /// Not a guard: treated as requiring a caller.
    fn guard_anonymous_ok(&self) -> bool {
        false
    }
}
impl<G> WithoutGuard for &Probe<G> {}

/// Fallback: no schema available.
pub trait WithoutToSchema {
    /// Documents nothing.
    fn schema_body(&self) -> Option<BodyDoc> {
        None
    }
}
impl<T> WithoutToSchema for &Probe<T> {}

/// `T: PartialSchema`: a path parameter gets a real schema.
pub trait ViaPartialSchema {
    /// The parameter's schema.
    fn path_schema(&self) -> RefOr<Schema>;
}
impl<T: PartialSchema> ViaPartialSchema for Probe<T> {
    fn path_schema(&self) -> RefOr<Schema> {
        T::schema()
    }
}
/// Fallback: documented as a string.
pub trait WithoutPartialSchema {
    /// A plain string schema.
    fn path_schema(&self) -> RefOr<Schema> {
        RefOr::T(Schema::Object(Object::with_type(SchemaType::Type(Type::String))))
    }
}
impl<T> WithoutPartialSchema for &Probe<T> {}

/// `T: IntoParams`: query parameters are documented.
pub trait ViaIntoParams {
    /// The query parameters of `T`.
    fn query_params(&self) -> Vec<Parameter>;
}
impl<T: IntoParams> ViaIntoParams for Probe<T> {
    fn query_params(&self) -> Vec<Parameter> {
        T::into_params(|| Some(ParameterIn::Query))
    }
}
/// Fallback: no query parameters are documented.
pub trait WithoutIntoParams {
    /// Documents nothing.
    fn query_params(&self) -> Vec<Parameter> {
        Vec::new()
    }
}
impl<T> WithoutIntoParams for &Probe<T> {}

// ------------------------------------------------------------------ assembly

/// The error envelope every failure uses (see `HttpError`).
#[derive(ToSchema)]
#[allow(dead_code, missing_docs)]
struct ErrorEnvelope {
    error: ErrorBody,
}

/// Body of [`ErrorEnvelope`].
#[derive(ToSchema)]
#[allow(dead_code, missing_docs)]
struct ErrorBody {
    /// Stable machine-readable code (`not_found`, `validation_failed`, ..).
    code: String,
    /// Human-readable message.
    message: String,
    /// Per-field details for validation failures.
    details: Option<serde_json::Value>,
}

const BEARER: &str = "bearerAuth";

/// Builds the OpenAPI document for `docs`, mounted under `prefix`.
pub(crate) fn build(info: &ApiInfo, prefix: &str, docs: &[RouteDoc]) -> OpenApi {
    let mut components = ComponentsBuilder::new()
        .schema(ErrorEnvelope::name(), ErrorEnvelope::schema())
        .schema(ErrorBody::name(), ErrorBody::schema())
        .security_scheme(
            BEARER,
            SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()),
        );

    let mut paths = PathsBuilder::new();
    let mut seen_schemas: BTreeMap<String, ()> = BTreeMap::new();
    for doc in docs {
        let (operation, schemas) = operation(doc);
        for (name, schema) in schemas {
            if seen_schemas.insert(name.clone(), ()).is_none() {
                components = components.schema(name, schema);
            }
        }
        let method = match doc.method {
            "get" => HttpMethod::Get,
            "post" => HttpMethod::Post,
            "put" => HttpMethod::Put,
            "patch" => HttpMethod::Patch,
            _ => HttpMethod::Delete,
        };
        let full = format!("{}{}", prefix.trim_end_matches('/'), doc.path);
        paths = paths.path(full, PathItem::new(method, operation));
    }

    let mut api_info = InfoBuilder::new().title(&info.title).version(&info.version);
    if let Some(description) = &info.description {
        api_info = api_info.description(Some(description.clone()));
    }
    OpenApiBuilder::new()
        .info(api_info.build())
        .paths(paths.build())
        .components(Some(components.build()))
        .build()
}

fn json(name: &str) -> ContentBuilder {
    ContentBuilder::new().schema(Some(RefOr::Ref(Ref::from_schema_name(name))))
}

fn json_body(body: &BodyDoc) -> ContentBuilder {
    if body.array {
        let items = RefOr::Ref(Ref::from_schema_name(&body.name));
        let array = ArrayBuilder::new().items(items).build();
        ContentBuilder::new().schema(Some(RefOr::T(Schema::Array(array))))
    } else {
        json(&body.name)
    }
}

/// A frame of the stream: `{ event: <name>, data, id?, retry? }`, the shape
/// oRPC's OpenAPI generator produces for event iterators.
fn frame(name: &str, data: RefOr<Schema>, optional_meta: bool) -> RefOr<Schema> {
    let mut object = ObjectBuilder::new()
        .property(
            "event",
            ObjectBuilder::new()
                .schema_type(SchemaType::Type(Type::String))
                .enum_values(Some([name])),
        )
        .required("event")
        .property("data", data)
        .required("data");
    if optional_meta {
        object = object
            .property("id", Object::with_type(SchemaType::Type(Type::String)))
            .property("retry", Object::with_type(SchemaType::Type(Type::Integer)));
    }
    RefOr::T(Schema::Object(object.build()))
}

fn event_stream_schema(message: RefOr<Schema>) -> RefOr<Schema> {
    let empty = || RefOr::T(Schema::Object(ObjectBuilder::new().build()));
    RefOr::T(Schema::OneOf(
        OneOfBuilder::new()
            .item(frame("message", message, true))
            .item(frame("close", empty(), false))
            .item(frame("error", empty(), false))
            .build(),
    ))
}

fn event_stream_content(body: &BodyDoc) -> ContentBuilder {
    let message = if body.array {
        let items = RefOr::Ref(Ref::from_schema_name(&body.name));
        RefOr::T(Schema::Array(ArrayBuilder::new().items(items).build()))
    } else {
        RefOr::Ref(Ref::from_schema_name(&body.name))
    };
    ContentBuilder::new().schema(Some(event_stream_schema(message)))
}

fn event_stream_content_untyped() -> ContentBuilder {
    ContentBuilder::new().schema(Some(event_stream_schema(RefOr::T(Schema::Object(
        ObjectBuilder::new().build(),
    )))))
}

fn error_response(description: &str) -> ResponseBuilder {
    ResponseBuilder::new()
        .description(description)
        .content("application/json", json(ErrorEnvelope::name().as_ref()).build())
}

/// The handler's description plus a line stating what the caller needs.
fn described(doc: &RouteDoc) -> Option<String> {
    if doc.requirements.is_empty() {
        return doc.description.clone();
    }
    let needs = format!("Requires {}.", doc.requirements.join("; "));
    Some(match &doc.description {
        Some(text) => format!("{text}\n\n{needs}"),
        None => needs,
    })
}

fn operation(doc: &RouteDoc) -> (Operation, Vec<(String, RefOr<Schema>)>) {
    let mut schemas = Vec::new();
    let mut builder = OperationBuilder::new()
        .tag(doc.controller)
        .operation_id(Some(format!("{}_{}", doc.controller, doc.handler)))
        .summary(doc.summary.clone())
        .description(described(doc));

    for (name, schema) in &doc.path_params {
        builder = builder.parameter(
            ParameterBuilder::new()
                .name(name)
                .parameter_in(ParameterIn::Path)
                .required(Required::True)
                .schema(Some(schema.clone())),
        );
    }
    for parameter in &doc.query {
        builder = builder.parameter(parameter.clone());
    }

    if let Some(body) = &doc.body {
        schemas.extend(body.schemas.clone());
        builder = builder.request_body(Some(
            RequestBodyBuilder::new()
                .required(Some(Required::True))
                .content("application/json", json_body(body).build())
                .build(),
        ));
    }

    for response in &doc.responses {
        let mut built = ResponseBuilder::new().description(reason(response.status));
        if let Some(body) = &response.body {
            schemas.extend(body.schemas.clone());
            if response.events {
                built = built.content("text/event-stream", event_stream_content(body).build());
            } else {
                built = built.content("application/json", json_body(body).build());
            }
        } else if response.events {
            built = built.content("text/event-stream", event_stream_content_untyped().build());
        }
        builder = builder.response(response.status.to_string(), built.build());
    }

    // Declared responses win over the generic ones the generator adds below.
    let declared = |status: u16| doc.declared_errors.iter().any(|(declared, _)| *declared == status);
    for (status, description) in &doc.declared_errors {
        builder = builder.response(status.to_string(), error_response(description).build());
    }
    if doc.secured || !doc.requirements.is_empty() {
        // In OpenAPI an empty requirement object means "no authentication is also fine".
        if doc.anonymous_ok && doc.requirements.is_empty() {
            builder = builder.security(SecurityRequirement::default());
        }
        builder = builder.security(SecurityRequirement::new(BEARER, Vec::<String>::new()));
        if !declared(401) {
            let meaning = if doc.anonymous_ok && doc.requirements.is_empty() {
                "A bearer token was sent but is invalid or expired"
            } else {
                "Missing or invalid bearer token"
            };
            builder = builder.response("401", error_response(meaning).build());
        }
    }
    if !doc.requirements.is_empty() && !declared(403) {
        builder = builder.response(
            "403",
            error_response("The caller lacks the required role or permission").build(),
        );
    }
    if doc.validated && !declared(422) {
        builder = builder.response("422", error_response("The request body failed validation").build());
    }
    if !doc.path_params.is_empty() && !declared(404) {
        builder = builder.response("404", error_response("Not found").build());
    }
    if doc.body.is_some() && !declared(400) {
        builder = builder.response("400", error_response("Malformed request").build());
    }

    (builder.build(), schemas)
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No content",
        _ => "Response",
    }
}
