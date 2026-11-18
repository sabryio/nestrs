//! The API as plain data: what the scanner finds and the emitter prints.
//!
//! Nothing here knows `syn` or TypeScript, so the two ends can change independently (DIP).

/// A Rust type, reduced to what a JSON contract can say about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeRef {
    /// Text.
    String,
    /// An integer; unsigned ones get a `min(0)`.
    Int {
        /// Whether the Rust type is unsigned.
        unsigned: bool,
    },
    /// A floating-point number.
    Float,
    /// `true` or `false`.
    Bool,
    /// A UUID string.
    Uuid,
    /// An ISO-8601 timestamp string.
    DateTime,
    /// `serde_json::Value`: anything.
    Any,
    /// A struct or enum defined in the project; its [`Schema`] is emitted once.
    Named(String),
    /// `Option<T>`: may be `null` (and, as a field, absent).
    Option(Box<TypeRef>),
    /// `Vec<T>`, sets, arrays.
    List(Box<TypeRef>),
    /// `HashMap<String, T>`.
    Map(Box<TypeRef>),
}

/// A `#[validate(..)]` rule the contract can enforce on the client too.
#[derive(Debug, Clone, PartialEq)]
pub enum Rule {
    /// `length(min, max)`: characters of a string or items of a list.
    Length {
        /// Inclusive lower bound.
        min: Option<u64>,
        /// Inclusive upper bound.
        max: Option<u64>,
    },
    /// `range(min, max)`.
    Range {
        /// Inclusive lower bound.
        min: Option<f64>,
        /// Inclusive upper bound.
        max: Option<f64>,
    },
    /// `email`.
    Email,
    /// `url`.
    Url,
}

/// One field of an object, named as it appears on the wire.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    /// The JSON key.
    pub name: String,
    /// The field's `///` text.
    pub docs: Option<String>,
    /// Its type.
    pub ty: TypeRef,
    /// May be missing from the JSON (`Option`, `#[serde(default)]`, `skip_serializing_if`).
    pub optional: bool,
    /// Validation the client can repeat.
    pub rules: Vec<Rule>,
}

/// One variant of an internally tagged enum.
#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    /// The value of the tag field that selects this variant.
    pub tag: String,
    /// The variant's `///` text.
    pub docs: Option<String>,
    /// Its fields besides the tag.
    pub fields: Vec<Field>,
}

/// The shape of a named type.
#[derive(Debug, Clone, PartialEq)]
pub enum SchemaKind {
    /// A JSON object.
    Object(Vec<Field>),
    /// A newtype (`struct Id(Uuid)`) is transparent to serde.
    Alias(TypeRef),
    /// An enum of unit variants: a string union.
    Enum(Vec<String>),
    /// `#[serde(tag = "..")]`.
    Tagged {
        /// Name of the discriminating field.
        tag: String,
        /// The alternatives.
        variants: Vec<Variant>,
    },
}

/// A named type the contract refers to.
#[derive(Debug, Clone, PartialEq)]
pub struct Schema {
    /// The Rust (and TypeScript) name.
    pub name: String,
    /// The type's `///` text.
    pub docs: Option<String>,
    /// Its shape.
    pub kind: SchemaKind,
}

/// What a handler receives besides the request itself.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Input {
    /// `{id}` segments, with their types, in path order.
    pub path: Vec<(String, TypeRef)>,
    /// Query-string fields.
    pub query: Vec<Field>,
    /// The JSON body.
    pub body: Option<TypeRef>,
}

/// What a handler returns on success.
#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    /// No body.
    Nothing,
    /// A JSON body.
    Json(TypeRef),
    /// A Server-Sent-Events stream of this message type.
    Events(TypeRef),
}

/// A documented failure: HTTP status and what it means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorCase {
    /// HTTP status.
    pub status: u16,
    /// What it means for this route.
    pub message: String,
}

/// One handler.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    /// camelCase method name.
    pub name: String,
    /// `GET`, `POST`, ..
    pub method: &'static str,
    /// Path below the API prefix, with `{param}` segments.
    pub path: String,
    /// First paragraph of the handler's docs.
    pub summary: Option<String>,
    /// The remaining paragraphs, plus any role or permission requirement.
    pub description: Option<String>,
    /// `#[status(..)]` when it is not 200.
    pub success_status: Option<u16>,
    /// Documented failures, by status.
    pub errors: Vec<ErrorCase>,
    /// What the handler takes.
    pub input: Input,
    /// What it returns.
    pub output: Output,
}

/// A controller: routes grouped under one key of the contract.
#[derive(Debug, Clone, PartialEq)]
pub struct Controller {
    /// `PostsController`.
    pub type_name: String,
    /// `posts`.
    pub key: String,
    /// Its handlers, in source order.
    pub routes: Vec<Route>,
}

/// Everything the emitter needs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Api {
    /// Where the server mounts the API (`/api`).
    pub prefix: String,
    /// Every controller, by key.
    pub controllers: Vec<Controller>,
    /// Dependencies first, so each declaration only refers to earlier ones.
    pub schemas: Vec<Schema>,
}
