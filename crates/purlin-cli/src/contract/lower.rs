//! From parsed Rust to the [`Api`] model: resolving types, reading serde and validator
//! attributes, understanding handler signatures.
//!
//! Anything the contract cannot express is an [`Unsupported`], never a guess. The caller turns
//! it into a warning and leaves that route out.

use std::collections::{BTreeMap, BTreeSet};

use syn::{Attribute, Fields, FnArg, GenericArgument, ImplItem, Meta, Pat, PathArguments, ReturnType, Type};

use super::model::*;
use super::scan::{Sources, TypeDef, doc_lines};
use super::serde_names::RenameRule;

/// Why something could not be written into the contract.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Unsupported(pub String);

fn unsupported<T>(message: impl Into<String>) -> Result<T, Unsupported> {
    Err(Unsupported(message.into()))
}

/// Extractors that carry no JSON contract (headers, request parts, framework state).
const IGNORED_EXTRACTORS: &[&str] = &[
    "LastEventId",
    "Request",
    "HeaderMap",
    "Parts",
    "Extension",
    "State",
    "Principal",
    "ConnectInfo",
    "TraceContext",
    "RequestId",
    "Inject",
    "ShuttingDown",
];

pub struct Lowerer<'a> {
    sources: &'a Sources,
    schemas: Vec<Schema>,
    done: BTreeSet<String>,
    in_progress: Vec<String>,
    pub warnings: Vec<String>,
}

impl<'a> Lowerer<'a> {
    pub fn new(sources: &'a Sources) -> Self {
        Self {
            sources,
            schemas: Vec::new(),
            done: BTreeSet::new(),
            in_progress: Vec::new(),
            warnings: Vec::new(),
        }
    }

    pub fn finish(self) -> (Vec<Schema>, Vec<String>) {
        (self.schemas, self.warnings)
    }

    // ------------------------------------------------------------------ types

    /// Resolves a Rust type, registering the schema of every named type it mentions.
    pub fn type_ref(&mut self, ty: &Type) -> Result<TypeRef, Unsupported> {
        match ty {
            Type::Reference(reference) => self.type_ref(&reference.elem),
            Type::Paren(inner) => self.type_ref(&inner.elem),
            Type::Group(inner) => self.type_ref(&inner.elem),
            Type::Array(array) => Ok(TypeRef::List(Box::new(self.type_ref(&array.elem)?))),
            Type::Slice(slice) => Ok(TypeRef::List(Box::new(self.type_ref(&slice.elem)?))),
            Type::Path(path) => {
                let segment = path.path.segments.last().expect("a path has a segment");
                let name = segment.ident.to_string();
                let args = generic_types(&segment.arguments);
                let one = |this: &mut Self| match args.first() {
                    Some(arg) => this.type_ref(arg),
                    None => unsupported(format!("`{name}` needs a type argument")),
                };
                Ok(match name.as_str() {
                    "String" | "str" | "Cow" | "PathBuf" => TypeRef::String,
                    "i8" | "i16" | "i32" | "i64" | "i128" | "isize" => TypeRef::Int { unsigned: false },
                    "u8" | "u16" | "u32" | "u64" | "u128" | "usize" => TypeRef::Int { unsigned: true },
                    "f32" | "f64" => TypeRef::Float,
                    "bool" => TypeRef::Bool,
                    "Uuid" => TypeRef::Uuid,
                    "DateTime" | "OffsetDateTime" | "NaiveDateTime" | "PrimitiveDateTime" => TypeRef::DateTime,
                    "Value" => TypeRef::Any,
                    "Box" | "Arc" | "Rc" => one(self)?,
                    "Option" => TypeRef::Option(Box::new(one(self)?)),
                    "Vec" | "VecDeque" | "HashSet" | "BTreeSet" => TypeRef::List(Box::new(one(self)?)),
                    "HashMap" | "BTreeMap" => match args.get(1) {
                        Some(value) => TypeRef::Map(Box::new(self.type_ref(value)?)),
                        None => return unsupported(format!("`{name}` needs key and value types")),
                    },
                    _ => {
                        self.named(&name)?;
                        TypeRef::Named(name)
                    }
                })
            }
            other => unsupported(format!("the type `{}` cannot be described in JSON", quote_type(other))),
        }
    }

    /// Makes sure `name`'s schema is registered (dependencies first).
    fn named(&mut self, name: &str) -> Result<(), Unsupported> {
        if self.done.contains(name) {
            return Ok(());
        }
        if self.sources.ambiguous.iter().any(|ambiguous| ambiguous == name) {
            return unsupported(format!(
                "`{name}` is defined in more than one module, so the contract cannot tell which one is meant"
            ));
        }
        if self.in_progress.iter().any(|open| open == name) {
            return unsupported(format!(
                "`{name}` is recursive, which the contract generator does not support"
            ));
        }
        let Some(def) = self.sources.types.get(name).cloned() else {
            return unsupported(format!(
                "`{name}` is not defined in this project (types from other crates need a wrapper that derives serde here)"
            ));
        };
        self.in_progress.push(name.to_owned());
        let schema = match &def {
            TypeDef::Struct(item) => self.structure(item),
            TypeDef::Enum(item) => self.enumeration(item),
        };
        self.in_progress.pop();
        let schema = schema.map_err(|Unsupported(why)| Unsupported(format!("{name}: {why}")))?;
        self.done.insert(name.to_owned());
        self.schemas.push(schema);
        Ok(())
    }

    fn structure(&mut self, item: &syn::ItemStruct) -> Result<Schema, Unsupported> {
        let container = Container::read(&item.attrs)?;
        let kind = match &item.fields {
            Fields::Named(named) => SchemaKind::Object(self.fields(named.named.iter(), &container)?),
            Fields::Unnamed(unnamed) if unnamed.unnamed.len() == 1 => {
                SchemaKind::Alias(self.type_ref(&unnamed.unnamed[0].ty)?)
            }
            _ => return unsupported("only structs with named fields (or a one-field newtype) are supported"),
        };
        Ok(Schema {
            name: item.ident.to_string(),
            docs: joined(&doc_lines(&item.attrs)),
            kind,
        })
    }

    fn enumeration(&mut self, item: &syn::ItemEnum) -> Result<Schema, Unsupported> {
        let container = Container::read(&item.attrs)?;
        if container.untagged {
            return unsupported("`#[serde(untagged)]` enums are not supported");
        }
        let unit_only = item
            .variants
            .iter()
            .all(|variant| matches!(variant.fields, Fields::Unit));
        let kind = match (&container.tag, unit_only) {
            (None, true) => SchemaKind::Enum(
                item.variants
                    .iter()
                    .map(|variant| container.variant_name(variant))
                    .collect::<Result<_, _>>()?,
            ),
            (None, false) => {
                return unsupported("an enum with data needs `#[serde(tag = \"..\")]` (internal tagging)");
            }
            (Some(tag), _) => {
                if container.content {
                    return unsupported("adjacently tagged enums (`content = ..`) are not supported");
                }
                let mut variants = Vec::new();
                for variant in &item.variants {
                    let fields = match &variant.fields {
                        Fields::Unit => Vec::new(),
                        Fields::Named(named) => self.fields(named.named.iter(), &Container::default())?,
                        Fields::Unnamed(_) => {
                            return unsupported(format!(
                                "variant `{}`: tuple variants are not supported; use named fields",
                                variant.ident
                            ));
                        }
                    };
                    variants.push(Variant {
                        tag: container.variant_name(variant)?,
                        docs: joined(&doc_lines(&variant.attrs)),
                        fields,
                    });
                }
                SchemaKind::Tagged {
                    tag: tag.clone(),
                    variants,
                }
            }
        };
        Ok(Schema {
            name: item.ident.to_string(),
            docs: joined(&doc_lines(&item.attrs)),
            kind,
        })
    }

    fn fields<'f>(
        &mut self,
        fields: impl Iterator<Item = &'f syn::Field>,
        container: &Container,
    ) -> Result<Vec<Field>, Unsupported> {
        let mut out = Vec::new();
        for field in fields {
            let attrs = FieldAttrs::read(&field.attrs)?;
            if attrs.skip {
                continue;
            }
            let ident = field.ident.as_ref().expect("named field").to_string();
            let ident = ident.strip_prefix("r#").unwrap_or(&ident).to_owned();
            let ty = self
                .type_ref(&field.ty)
                .map_err(|Unsupported(why)| Unsupported(format!("field `{ident}`: {why}")))?;
            let name = attrs
                .rename
                .clone()
                .unwrap_or_else(|| container.rename_all.map_or(ident.clone(), |rule| rule.apply(&ident)));
            out.push(Field {
                name,
                docs: joined(&doc_lines(&field.attrs)),
                optional: matches!(ty, TypeRef::Option(_)) || attrs.default || attrs.skip_if,
                ty,
                rules: validation_rules(&field.attrs),
            });
        }
        Ok(out)
    }

    // ----------------------------------------------------------------- routes

    pub fn controller(&mut self, item: &syn::ItemImpl) -> Option<Controller> {
        let Type::Path(self_ty) = &*item.self_ty else {
            return None;
        };
        let type_name = self_ty.path.segments.last()?.ident.to_string();
        let controller_attr = item.attrs.iter().find(|attr| attr.path().is_ident("controller"))?;
        let prefix = controller_attr
            .parse_args::<syn::LitStr>()
            .map(|text| text.value())
            .unwrap_or_default();
        let class = Policy::read(&item.attrs);

        let mut routes = Vec::new();
        for member in &item.items {
            let ImplItem::Fn(method) = member else { continue };
            let Some((verb, route_path)) = route_attr(&method.attrs) else {
                continue;
            };
            let label = format!("{type_name}::{}", method.sig.ident);
            match self.route(method, verb, &join_paths(&[&prefix, &route_path]), &class) {
                Ok(route) => routes.push(route),
                Err(Unsupported(why)) => self.warnings.push(format!("{label} left out of the contract: {why}")),
            }
        }
        let key = camel(type_name.strip_suffix("Controller").unwrap_or(&type_name));
        Some(Controller { type_name, key, routes })
    }

    fn route(
        &mut self,
        method: &syn::ImplItemFn,
        verb: &'static str,
        path: &str,
        class: &Policy,
    ) -> Result<Route, Unsupported> {
        let own = Policy::read(&method.attrs);
        let guards: Vec<&String> = class.guards.iter().chain(&own.guards).collect();
        let mut requirements = class.requirements.clone();
        requirements.extend(own.requirements.clone());

        let mut input = Input::default();
        let mut validated = false;
        let placeholders = placeholders(path);
        for arg in &method.sig.inputs {
            let FnArg::Typed(arg) = arg else { continue };
            self.extractor(&arg.ty, &placeholders, &mut input, &mut validated, &arg.pat)?;
        }
        let output = self.output(&method.sig.output)?;

        let docs = doc_lines(&method.attrs);
        let (summary, mut description) = split_docs(&docs);
        if let Some(requires) = requirement_text(&requirements) {
            description = Some(match description {
                Some(text) => format!("{text}\n\n{requires}"),
                None => requires,
            });
        }

        let mut errors: BTreeMap<u16, String> = BTreeMap::new();
        if input.body.is_some() {
            errors.insert(400, "Malformed request".to_owned());
        }
        if !guards.is_empty() {
            let bearer = guards.iter().any(|guard| guard.as_str() == "BearerAuth");
            errors.insert(
                401,
                if bearer {
                    "Missing or invalid bearer token"
                } else {
                    "Missing or invalid credentials"
                }
                .to_owned(),
            );
        }
        if !requirements.is_empty() {
            errors.insert(403, "The caller lacks the required role or permission".to_owned());
        }
        if validated {
            errors.insert(422, "The request body failed validation".to_owned());
        }
        for (status, message) in own.responses.iter().chain(&class.responses) {
            errors.insert(*status, message.clone());
        }
        if let Some(status) = errors.keys().find(|status| super::emit::error_code(**status).is_none()) {
            return unsupported(format!("status {status} has no oRPC error code"));
        }

        Ok(Route {
            name: camel(&method.sig.ident.to_string()),
            method: verb,
            path: path.to_owned(),
            summary,
            description,
            success_status: own.status.filter(|status| *status != 200),
            errors: errors
                .into_iter()
                .map(|(status, message)| ErrorCase { status, message })
                .collect(),
            input,
            output,
        })
    }

    fn extractor(
        &mut self,
        ty: &Type,
        placeholders: &[String],
        input: &mut Input,
        validated: &mut bool,
        pattern: &Pat,
    ) -> Result<(), Unsupported> {
        let Type::Path(path) = ty else {
            return Ok(());
        };
        let segment = path.path.segments.last().expect("a path has a segment");
        let name = segment.ident.to_string();
        let args = generic_types(&segment.arguments);
        match (name.as_str(), args.first()) {
            ("Path", Some(inner)) => self.path_params(inner, placeholders, input)?,
            ("Query", Some(inner)) => input.query.extend(self.query_fields(inner)?),
            ("Json" | "ValidatedJson", Some(inner)) => {
                if input.body.is_some() {
                    return unsupported("a handler can take only one JSON body");
                }
                input.body = Some(self.type_ref(inner)?);
                *validated |= name == "ValidatedJson";
            }
            ("Piped", Some(Type::Path(pipe))) if last_ident(pipe) == "PaginationPipe" => {
                for field in ["limit", "offset"] {
                    input.query.push(Field {
                        name: field.to_owned(),
                        docs: Some(
                            if field == "limit" {
                                "Page size, 1 to 100 (default 20)."
                            } else {
                                "Items to skip (default 0)."
                            }
                            .to_owned(),
                        ),
                        ty: TypeRef::Int { unsigned: true },
                        optional: true,
                        rules: Vec::new(),
                    });
                }
            }
            _ if IGNORED_EXTRACTORS.contains(&name.as_str()) => {}
            _ => self.warnings.push(format!(
                "parameter `{}: {name}` is not part of the contract (only Path, Query, Json, ValidatedJson and PaginationPipe are)",
                pattern_name(pattern)
            )),
        }
        Ok(())
    }

    fn path_params(&mut self, inner: &Type, placeholders: &[String], input: &mut Input) -> Result<(), Unsupported> {
        match inner {
            Type::Tuple(tuple) => {
                if tuple.elems.len() != placeholders.len() {
                    return unsupported(format!(
                        "`Path<(..)>` has {} elements but the route has {} `{{param}}` segments",
                        tuple.elems.len(),
                        placeholders.len()
                    ));
                }
                for (name, ty) in placeholders.iter().zip(&tuple.elems) {
                    input.path.push((name.clone(), self.type_ref(ty)?));
                }
            }
            other => {
                let resolved = self.type_ref(other)?;
                if let TypeRef::Named(name) = &resolved
                    && let Some(Schema {
                        kind: SchemaKind::Object(fields),
                        ..
                    }) = self.schemas.iter().find(|schema| &schema.name == name).cloned()
                {
                    for field in fields {
                        input.path.push((field.name, field.ty));
                    }
                } else if placeholders.len() == 1 {
                    input.path.push((placeholders[0].clone(), resolved));
                } else {
                    return unsupported(format!(
                        "`Path<T>` with a single value needs exactly one `{{param}}` segment, found {}",
                        placeholders.len()
                    ));
                }
            }
        }
        Ok(())
    }

    fn query_fields(&mut self, inner: &Type) -> Result<Vec<Field>, Unsupported> {
        let resolved = self.type_ref(inner)?;
        match &resolved {
            TypeRef::Named(name) => match self.schemas.iter().find(|schema| &schema.name == name) {
                Some(Schema {
                    kind: SchemaKind::Object(fields),
                    ..
                }) => Ok(fields.clone()),
                _ => unsupported(format!("`Query<{name}>` must be a struct with named fields")),
            },
            _ => unsupported("`Query<T>` must be a struct with named fields"),
        }
    }

    fn output(&mut self, output: &ReturnType) -> Result<Output, Unsupported> {
        let ReturnType::Type(_, ty) = output else {
            return Ok(Output::Nothing);
        };
        let mut ty: &Type = ty;
        if let Type::Path(path) = ty
            && let Some(segment) = path.path.segments.last()
            && segment.ident == "Result"
            && let Some(ok) = generic_types(&segment.arguments).into_iter().next()
        {
            ty = ok;
        }
        match ty {
            Type::Tuple(tuple) if tuple.elems.is_empty() => Ok(Output::Nothing),
            // `(headers, Json<T>)`: the other elements are response parts, not body.
            Type::Tuple(tuple) => {
                let bodies: Vec<&Type> = tuple
                    .elems
                    .iter()
                    .filter(|elem| matches!(elem, Type::Path(p) if matches!(last_ident(p).as_str(), "Json" | "EventStream")))
                    .collect();
                match bodies.as_slice() {
                    [body] => self.output(&ReturnType::Type(Default::default(), Box::new((*body).clone()))),
                    [] => Ok(Output::Nothing),
                    _ => unsupported("a tuple return with more than one body"),
                }
            }
            Type::Path(path) => {
                let segment = path.path.segments.last().expect("a path has a segment");
                let inner = generic_types(&segment.arguments).into_iter().next();
                match (segment.ident.to_string().as_str(), inner) {
                    ("Json", Some(inner)) => Ok(Output::Json(self.type_ref(inner)?)),
                    ("EventStream", Some(inner)) => Ok(Output::Events(self.type_ref(inner)?)),
                    ("StatusCode", _) => Ok(Output::Nothing),
                    (other, _) => unsupported(format!(
                        "the return type `{other}` is not understood; return `Json<T>`, `EventStream<T>`, `()` or a `Result` of them"
                    )),
                }
            }
            other => unsupported(format!(
                "the return type `{}` is not understood; return `Json<T>`, `EventStream<T>`, `()` or a `Result` of them",
                quote_type(other)
            )),
        }
    }
}

// ------------------------------------------------------------------ helpers

fn generic_types(arguments: &PathArguments) -> Vec<&Type> {
    match arguments {
        PathArguments::AngleBracketed(angle) => angle
            .args
            .iter()
            .filter_map(|arg| match arg {
                GenericArgument::Type(ty) => Some(ty),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn last_ident(path: &syn::TypePath) -> String {
    path.path
        .segments
        .last()
        .map_or_else(String::new, |segment| segment.ident.to_string())
}

fn quote_type(ty: &Type) -> String {
    use quote_like::Render;
    ty.render()
}

/// Minimal type printing for messages, without a `quote` dependency.
mod quote_like {
    use syn::Type;

    pub trait Render {
        fn render(&self) -> String;
    }

    impl Render for Type {
        fn render(&self) -> String {
            match self {
                Type::Tuple(tuple) => format!(
                    "({})",
                    tuple.elems.iter().map(Render::render).collect::<Vec<_>>().join(", ")
                ),
                Type::Path(path) => path
                    .path
                    .segments
                    .iter()
                    .map(|segment| segment.ident.to_string())
                    .collect::<Vec<_>>()
                    .join("::"),
                Type::ImplTrait(_) => "impl Trait".to_owned(),
                Type::TraitObject(_) => "dyn Trait".to_owned(),
                _ => "this type".to_owned(),
            }
        }
    }
}

fn pattern_name(pattern: &Pat) -> String {
    match pattern {
        Pat::Ident(ident) => ident.ident.to_string(),
        _ => "_".to_owned(),
    }
}

fn joined(lines: &[String]) -> Option<String> {
    let text = lines.join("\n").trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// The first paragraph is the summary (one line); the rest is the description.
fn split_docs(lines: &[String]) -> (Option<String>, Option<String>) {
    let text = lines.join("\n");
    let text = text.trim();
    if text.is_empty() {
        return (None, None);
    }
    let (first, rest) = text.split_once("\n\n").unwrap_or((text, ""));
    let summary = first.lines().map(str::trim).collect::<Vec<_>>().join(" ");
    let rest = rest.trim();
    (Some(summary), (!rest.is_empty()).then(|| rest.to_owned()))
}

fn requirement_text(requirements: &[Requirement]) -> Option<String> {
    let parts: Vec<String> = requirements
        .iter()
        .map(|requirement| match requirement.kind {
            RequirementKind::Roles => format!("Requires role {}.", requirement.names.join(" or ")),
            RequirementKind::Permissions => format!("Requires permission {}.", requirement.names.join(" and ")),
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// `/a/{id}/b` → `["id"]`.
pub fn placeholders(path: &str) -> Vec<String> {
    path.split('/')
        .filter_map(|segment| segment.strip_prefix('{')?.strip_suffix('}'))
        .map(|name| name.trim_start_matches('*').to_owned())
        .collect()
}

fn join_paths(parts: &[&str]) -> String {
    let joined: Vec<&str> = parts
        .iter()
        .flat_map(|part| part.split('/'))
        .filter(|segment| !segment.is_empty())
        .collect();
    format!("/{}", joined.join("/"))
}

/// `list_all` → `listAll`; `PostsApi` → `postsApi`.
pub fn camel(name: &str) -> String {
    let mut out = String::new();
    let mut upper_next = false;
    for (index, ch) in name.chars().enumerate() {
        if ch == '_' {
            upper_next = true;
        } else if upper_next {
            out.extend(ch.to_uppercase());
            upper_next = false;
        } else if index == 0 {
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// `#[get]`, `#[get("/x")]` → (`GET`, `/x`).
fn route_attr(attrs: &[Attribute]) -> Option<(&'static str, String)> {
    for (name, verb) in [
        ("get", "GET"),
        ("post", "POST"),
        ("put", "PUT"),
        ("patch", "PATCH"),
        ("delete", "DELETE"),
    ] {
        if let Some(attr) = attrs.iter().find(|attr| attr.path().is_ident(name)) {
            let path = match &attr.meta {
                Meta::List(_) => attr
                    .parse_args::<syn::LitStr>()
                    .map(|text| text.value())
                    .unwrap_or_default(),
                _ => String::new(),
            };
            return Some((verb, path));
        }
    }
    None
}

// ---------------------------------------------------------- attribute reading

#[derive(Clone, Copy, PartialEq, Eq)]
enum RequirementKind {
    Roles,
    Permissions,
}

#[derive(Clone)]
struct Requirement {
    kind: RequirementKind,
    names: Vec<String>,
}

/// What `#[guard]`, `#[roles]`, `#[status]` and `#[response]` say about a route or controller.
#[derive(Default)]
struct Policy {
    guards: Vec<String>,
    requirements: Vec<Requirement>,
    status: Option<u16>,
    responses: Vec<(u16, String)>,
}

impl Policy {
    fn read(attrs: &[Attribute]) -> Self {
        let mut policy = Self::default();
        for attr in attrs {
            let Some(ident) = attr.path().get_ident().map(ToString::to_string) else {
                continue;
            };
            match ident.as_str() {
                "guard" => policy
                    .guards
                    .push(attr.parse_args::<Type>().map(|ty| quote_type(&ty)).unwrap_or_default()),
                "roles" | "permissions" => {
                    let names = attr
                        .parse_args_with(syn::punctuated::Punctuated::<syn::LitStr, syn::Token![,]>::parse_terminated)
                        .map(|list| list.iter().map(syn::LitStr::value).collect())
                        .unwrap_or_default();
                    policy.requirements.push(Requirement {
                        kind: if ident == "roles" {
                            RequirementKind::Roles
                        } else {
                            RequirementKind::Permissions
                        },
                        names,
                    });
                }
                "status" => {
                    policy.status = attr
                        .parse_args::<syn::LitInt>()
                        .ok()
                        .and_then(|lit| lit.base10_parse().ok());
                }
                "response" => {
                    let parsed = attr.parse_args_with(|input: syn::parse::ParseStream| {
                        let code: syn::LitInt = input.parse()?;
                        input.parse::<syn::Token![,]>()?;
                        let text: syn::LitStr = input.parse()?;
                        Ok((code.base10_parse::<u16>()?, text.value()))
                    });
                    if let Ok(response) = parsed {
                        policy.responses.push(response);
                    }
                }
                _ => {}
            }
        }
        policy
    }
}

/// `#[serde(..)]` on a struct or enum.
#[derive(Default)]
struct Container {
    rename_all: Option<RenameRule>,
    tag: Option<String>,
    content: bool,
    untagged: bool,
}

impl Container {
    fn read(attrs: &[Attribute]) -> Result<Self, Unsupported> {
        let mut container = Self::default();
        for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
            let mut failure = None;
            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename_all") {
                    let value: syn::LitStr = meta.value()?.parse()?;
                    match RenameRule::parse(&value.value()) {
                        Some(rule) => container.rename_all = Some(rule),
                        None => failure = Some(format!("unknown rename_all rule {:?}", value.value())),
                    }
                } else if meta.path.is_ident("tag") {
                    container.tag = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                } else if meta.path.is_ident("content") {
                    meta.value()?.parse::<syn::LitStr>()?;
                    container.content = true;
                } else if meta.path.is_ident("untagged") {
                    container.untagged = true;
                } else {
                    skip_value(&meta)?;
                }
                Ok(())
            });
            if let Some(message) = failure {
                return unsupported(message);
            }
        }
        Ok(container)
    }

    fn variant_name(&self, variant: &syn::Variant) -> Result<String, Unsupported> {
        let mut rename = None;
        for attr in variant.attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename") {
                    rename = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                } else {
                    skip_value(&meta)?;
                }
                Ok(())
            });
        }
        let ident = variant.ident.to_string();
        Ok(rename.unwrap_or_else(|| self.rename_all.map_or(ident.clone(), |rule| rule.apply(&ident))))
    }
}

/// `#[serde(..)]` on a field.
#[derive(Default)]
struct FieldAttrs {
    rename: Option<String>,
    skip: bool,
    default: bool,
    skip_if: bool,
}

impl FieldAttrs {
    fn read(attrs: &[Attribute]) -> Result<Self, Unsupported> {
        let mut read = Self::default();
        let mut flatten = false;
        for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename") {
                    read.rename = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                } else if meta.path.is_ident("skip") {
                    read.skip = true;
                } else if meta.path.is_ident("default") {
                    skip_value(&meta)?;
                    read.default = true;
                } else if meta.path.is_ident("skip_serializing_if") {
                    skip_value(&meta)?;
                    read.skip_if = true;
                } else if meta.path.is_ident("flatten") {
                    flatten = true;
                } else {
                    skip_value(&meta)?;
                }
                Ok(())
            });
        }
        if flatten {
            return unsupported("`#[serde(flatten)]` is not supported");
        }
        Ok(read)
    }
}

/// Consumes the `= value` or `(..)` after an attribute key this reader does not care about.
fn skip_value(meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<()> {
    if meta.input.peek(syn::Token![=]) {
        meta.value()?.parse::<syn::Expr>()?;
    } else if meta.input.peek(syn::token::Paren) {
        meta.parse_nested_meta(|inner| skip_value(&inner))?;
    }
    Ok(())
}

/// The `validator` rules the client can enforce too.
fn validation_rules(attrs: &[Attribute]) -> Vec<Rule> {
    let mut rules = Vec::new();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("validate")) {
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("email") {
                rules.push(Rule::Email);
                skip_value(&meta)?;
            } else if meta.path.is_ident("url") {
                rules.push(Rule::Url);
                skip_value(&meta)?;
            } else if meta.path.is_ident("length") || meta.path.is_ident("range") {
                let is_length = meta.path.is_ident("length");
                let (mut min, mut max) = (None, None);
                meta.parse_nested_meta(|bound| {
                    let slot = if bound.path.is_ident("min") {
                        Some(&mut min)
                    } else if bound.path.is_ident("max") {
                        Some(&mut max)
                    } else {
                        None
                    };
                    let value: syn::Expr = bound.value()?.parse()?;
                    if let Some(slot) = slot
                        && let Some(number) = number(&value)
                    {
                        *slot = Some(number);
                    }
                    Ok(())
                })?;
                rules.push(if is_length {
                    Rule::Length {
                        min: min.map(|n| n as u64),
                        max: max.map(|n| n as u64),
                    }
                } else {
                    Rule::Range { min, max }
                });
            } else {
                skip_value(&meta)?;
            }
            Ok(())
        });
    }
    rules
}

fn number(expr: &syn::Expr) -> Option<f64> {
    match expr {
        syn::Expr::Lit(syn::ExprLit { lit, .. }) => match lit {
            syn::Lit::Int(int) => int.base10_parse().ok(),
            syn::Lit::Float(float) => float.base10_parse().ok(),
            _ => None,
        },
        syn::Expr::Unary(unary) if matches!(unary.op, syn::UnOp::Neg(_)) => number(&unary.expr).map(|n| -n),
        _ => None,
    }
}
