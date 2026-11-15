use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{Attribute, Expr, FnArg, ImplItem, ItemImpl, LitStr, Meta, Type};

/// `#[roles("a", "b")]` (any of) or `#[permissions("x", "y")]` (all of).
struct Requirement {
    kind: RequirementKind,
    names: Vec<String>,
    span: proc_macro2::Span,
}

#[derive(Clone, Copy, PartialEq)]
enum RequirementKind {
    Roles,
    Permissions,
}

impl Requirement {
    /// The same wording as `nestrs_http::Requirement`'s `Display`, for OpenAPI.
    fn describe(&self) -> String {
        match self.kind {
            RequirementKind::Roles => format!("role {}", self.names.join(" or ")),
            RequirementKind::Permissions => format!("permission {}", self.names.join(" and ")),
        }
    }

    fn construct(&self) -> TokenStream {
        let names = &self.names;
        match self.kind {
            RequirementKind::Roles => quote!(::nestrs::http::Requirement::AnyRole(&[#(#names),*])),
            RequirementKind::Permissions => quote!(::nestrs::http::Requirement::AllPermissions(&[#(#names),*])),
        }
    }
}

/// One `#[interceptor(..)]`: a provider type resolved from the container, or an
/// expression evaluated once where the controller is mounted (`RateLimit::per_minute(60)`).
enum Interceptor {
    Injected(Type),
    Inline(Expr),
}

impl Interceptor {
    fn parse(attr: &Attribute) -> syn::Result<Self> {
        attr.parse_args_with(|input: syn::parse::ParseStream| {
            let fork = input.fork();
            // A bare type (`Timing`, `crate::Timing`) is a provider; anything else is a value.
            if fork.parse::<Type>().is_ok() && fork.is_empty() {
                return input.parse().map(Self::Injected);
            }
            input.parse().map(Self::Inline)
        })
    }

    fn span(&self) -> proc_macro2::Span {
        match self {
            Self::Injected(ty) => ty.span(),
            Self::Inline(expr) => expr.span(),
        }
    }
}

const VERBS: [&str; 5] = ["get", "post", "put", "patch", "delete"];

struct Route {
    verb: &'static str,
    path: String,
    method: syn::Ident,
    extractors: Vec<Type>,
    guards: Vec<Type>,
    interceptors: Vec<Interceptor>,
    requirements: Vec<Requirement>,
    /// The handler's `///` lines, for OpenAPI summaries.
    doc_lines: Vec<String>,
    output: syn::ReturnType,
    /// `#[status(201)]`: the default success status (applied and documented).
    status: Option<u16>,
    /// `#[response(404, "Not found")]`: extra documented error responses.
    responses: Vec<(u16, String)>,
}

pub(crate) fn expand(args: TokenStream, mut item: ItemImpl) -> syn::Result<TokenStream> {
    if item.trait_.is_some() {
        return Err(syn::Error::new_spanned(
            &item.self_ty,
            "#[controller] applies to an inherent `impl`, not a trait impl",
        ));
    }
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "#[controller] does not support generic impl blocks",
        ));
    }

    let prefix = if args.is_empty() {
        String::new()
    } else {
        syn::parse2::<LitStr>(args)?.value()
    };

    let class_guards = take_typed(&mut item.attrs, "guard")?;
    let class_interceptors = take_interceptors(&mut item.attrs)?;
    let class_requirements = take_requirements(&mut item.attrs)?;
    let mut routes = Vec::new();

    for member in &mut item.items {
        let ImplItem::Fn(method) = member else { continue };

        let route = take_route(&mut method.attrs)?;
        let guards = take_typed(&mut method.attrs, "guard")?;
        let interceptors = take_interceptors(&mut method.attrs)?;
        let requirements = take_requirements(&mut method.attrs)?;
        let status = take_status(&mut method.attrs)?;
        let responses = take_responses(&mut method.attrs)?;
        let Some((verb, path)) = route else {
            if let Some((_, span)) = status {
                return Err(syn::Error::new(
                    span,
                    "#[status] on a method that has no #[get]/#[post]/#[put]/#[patch]/#[delete]",
                ));
            }
            if let Some((_, _, span)) = responses.first() {
                return Err(syn::Error::new(
                    *span,
                    "#[response] on a method that has no #[get]/#[post]/#[put]/#[patch]/#[delete]",
                ));
            }
            if let Some(guard) = guards.first() {
                return Err(syn::Error::new_spanned(
                    guard,
                    "#[guard] on a method that has no #[get]/#[post]/#[put]/#[patch]/#[delete]",
                ));
            }
            if let Some(requirement) = requirements.first() {
                return Err(syn::Error::new(
                    requirement.span,
                    "#[roles]/#[permissions] on a method that has no #[get]/#[post]/#[put]/#[patch]/#[delete]",
                ));
            }
            if let Some(interceptor) = interceptors.first() {
                return Err(syn::Error::new(
                    interceptor.span(),
                    "#[interceptor] on a method that has no #[get]/#[post]/#[put]/#[patch]/#[delete]",
                ));
            }
            continue;
        };

        let signature = &method.sig;
        if signature.asyncness.is_none() {
            return Err(syn::Error::new_spanned(
                signature.fn_token,
                "route handlers must be `async fn`",
            ));
        }
        match signature.inputs.first() {
            Some(FnArg::Receiver(receiver)) if crate::util::is_shared_self(receiver) => {}
            _ => {
                return Err(syn::Error::new_spanned(
                    &signature.inputs,
                    "route handlers must take `&self` as their first parameter",
                ));
            }
        }

        let extractors = signature
            .inputs
            .iter()
            .skip(1)
            .map(|argument| match argument {
                FnArg::Typed(typed) if last_segment_is(&typed.ty, "State") => Err(syn::Error::new_spanned(
                    &typed.ty,
                    "nestrs controllers have no axum `State`: share services by injecting them as fields of the \
                     controller, or take `Inject<T>` for a single handler",
                )),
                FnArg::Typed(typed) => Ok((*typed.ty).clone()),
                FnArg::Receiver(receiver) => Err(syn::Error::new_spanned(receiver, "unexpected receiver")),
            })
            .collect::<syn::Result<Vec<_>>>()?;

        let doc_lines = doc_lines(&method.attrs);

        routes.push(Route {
            verb,
            path,
            method: signature.ident.clone(),
            extractors,
            guards,
            interceptors,
            requirements,
            doc_lines,
            output: signature.output.clone(),
            status: status.map(|(code, _)| code),
            responses: responses.into_iter().map(|(code, text, _)| (code, text)).collect(),
        });
    }

    if routes.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.self_ty,
            "#[controller] found no routes; annotate methods with #[get], #[post], #[put], #[patch] or #[delete]",
        ));
    }

    let self_ty = &item.self_ty;
    // Guards and interceptors are resolved when routes are mounted; declaring them as
    // extra dependencies makes bootstrap check that they exist and are visible.
    let guard_tokens = class_guards
        .iter()
        .chain(class_interceptors.iter().filter_map(injected))
        .chain(routes.iter().flat_map(|route| {
            route
                .guards
                .iter()
                .chain(route.interceptors.iter().filter_map(injected))
        }))
        .map(|ty| quote!(::nestrs::Token::of::<#ty>()));

    // Class-level inline interceptors are built once and shared by every route, so a
    // class-level limit is one allowance for the whole controller.
    let class_inline = class_interceptors
        .iter()
        .enumerate()
        .filter_map(|(i, interceptor)| match interceptor {
            Interceptor::Inline(expr) => {
                let name = format_ident!("__class_interceptor{i}");
                let build = inline_interceptor(expr);
                Some(quote!(let #name = #build;))
            }
            Interceptor::Injected(_) => None,
        });

    // `Inject<T>` and `Piped<P>` parameters resolve their provider per request.
    let request_tokens = routes
        .iter()
        .flat_map(|route| route.extractors.iter())
        .filter_map(request_scoped_target)
        .map(|ty| quote!(::nestrs::Token::of::<#ty>()));

    let statements = routes.iter().map(|route| {
        let method = &route.method;
        let path = &route.path;
        let verb = format_ident!("{}", route.verb);
        let names: Vec<_> = (0..route.extractors.len()).map(|i| format_ident!("__arg{i}")).collect();
        let types = &route.extractors;

        // Guards run in declaration order, class-level first. Each `route_layer`
        // wraps the previous one, so apply them last-to-first.
        // Interceptors sit closest to the handler, guards outside them, so the order
        // is: guards -> interceptors -> handler (as in NestJS).
        let guards: Vec<&Type> = class_guards.iter().chain(route.guards.iter()).collect();
        let interceptors: Vec<(Option<usize>, &Interceptor)> = class_interceptors
            .iter()
            .enumerate()
            .map(|(i, interceptor)| (Some(i), interceptor))
            .chain(route.interceptors.iter().map(|interceptor| (None, interceptor)))
            .collect();
        let interceptor_layers = interceptors.iter().rev().map(|(class_index, interceptor)| {
            let instance = match (interceptor, class_index) {
                (Interceptor::Injected(ty), _) => quote!(__container.resolve::<#ty>()?),
                (Interceptor::Inline(_), Some(i)) => {
                    let name = format_ident!("__class_interceptor{i}");
                    quote!(::std::sync::Arc::clone(&#name))
                }
                (Interceptor::Inline(expr), None) => inline_interceptor(expr),
            };
            quote! {
                __route = ::nestrs::http::intercepted(__route, #instance);
            }
        });
        // Requirements sit between guards (which authenticate) and interceptors.
        let requirement_layers = class_requirements
            .iter()
            .chain(route.requirements.iter())
            .rev()
            .map(|requirement| {
                let requirement = requirement.construct();
                quote!(__route = ::nestrs::http::authorized(__route, #requirement);)
            });
        let guard_layers = guards.iter().rev().map(|guard| {
            quote! {
                __route = ::nestrs::http::guarded(__route, __container.resolve::<#guard>()?);
            }
        });

        // One assertion per parameter, spanned on the user's type, so the compiler
        // points at the offending parameter instead of the whole impl block.
        let last = route.extractors.len().saturating_sub(1);
        let assertions = route.extractors.iter().enumerate().map(|(i, ty)| {
            if i == last {
                quote_spanned!(ty.span()=> ::nestrs::http::assert_last_extractor::<#ty, _>();)
            } else {
                quote_spanned!(ty.span()=> ::nestrs::http::assert_parts_extractor::<#ty>();)
            }
        });

        // `#[status(..)]` turns a default 200 into the declared success status.
        let call = match route.status {
            Some(code) => quote! {
                ::nestrs::http::with_default_status(#code, __controller.#method(#(#names),*).await)
            },
            None => quote!(__controller.#method(#(#names),*).await),
        };

        quote! {
            {
                #(#assertions)*
                let __controller = ::std::sync::Arc::clone(&self);
                let __handler = move |#(#names: #types),*| {
                    let __controller = ::std::sync::Arc::clone(&__controller);
                    async move { #call }
                };
                let mut __route = ::nestrs::http::axum::routing::#verb(__handler);
                #(#interceptor_layers)*
                #(#requirement_layers)*
                #(#guard_layers)*
                __router = __router.route(
                    &::nestrs::http::join_path(<Self as ::nestrs::http::HttpController>::PREFIX, #path),
                    __route,
                );
            }
        }
    });

    let route_docs = routes
        .iter()
        .map(|route| route_doc(route, &class_requirements, &class_guards, &self_ty_name(&item.self_ty)));

    Ok(quote! {
        #item

        impl ::nestrs::http::HttpController for #self_ty {
            const PREFIX: &'static str = #prefix;

            fn guard_tokens() -> ::std::vec::Vec<::nestrs::Token> {
                ::std::vec![#(#guard_tokens),*]
            }

            fn request_tokens() -> ::std::vec::Vec<::nestrs::Token> {
                ::std::vec![#(#request_tokens),*]
            }

            fn docs() -> ::std::vec::Vec<::nestrs::http::openapi::RouteDoc> {
                ::std::vec![#(#route_docs),*]
            }

            fn routes(
                self: ::std::sync::Arc<Self>,
                __container: &::nestrs::Container,
            ) -> ::std::result::Result<::nestrs::http::axum::Router, ::nestrs::DiError> {
                let mut __router = ::nestrs::http::axum::Router::new();
                #(#class_inline)*
                #(#statements)*
                ::std::result::Result::Ok(__router)
            }
        }

        impl ::nestrs::Controller for #self_ty {
            fn entry() -> ::nestrs::ControllerEntry {
                ::nestrs::http::mount::<Self>()
            }
        }
    })
}

/// Pulls a single `#[get("/x")]`-style attribute out of `attrs`.
fn take_route(attrs: &mut Vec<Attribute>) -> syn::Result<Option<(&'static str, String)>> {
    let mut found: Option<(&'static str, String)> = None;
    let mut failure: Option<syn::Error> = None;

    attrs.retain(|attr| {
        let Some(verb) = VERBS.iter().find(|verb| attr.path().is_ident(verb)) else {
            return true;
        };
        if found.is_some() {
            failure = Some(syn::Error::new_spanned(
                attr,
                "a handler can have only one route attribute",
            ));
            return false;
        }
        let path = match &attr.meta {
            Meta::Path(_) => Ok("/".to_owned()),
            Meta::List(_) => attr.parse_args::<LitStr>().map(|lit| lit.value()),
            Meta::NameValue(_) => Err(syn::Error::new_spanned(
                attr,
                "use `#[get(\"/path\")]`, not `#[get = \"/path\"]`",
            )),
        };
        match path {
            Ok(path) => found = Some((verb, path)),
            Err(error) => failure = Some(error),
        }
        false
    });

    match failure {
        Some(error) => Err(error),
        None => Ok(found),
    }
}

fn injected(interceptor: &Interceptor) -> Option<&Type> {
    match interceptor {
        Interceptor::Injected(ty) => Some(ty),
        Interceptor::Inline(_) => None,
    }
}

/// `Arc::new(expr)`, checked against `Interceptor` with the error spanned on `expr`.
fn inline_interceptor(expr: &Expr) -> TokenStream {
    quote_spanned!(expr.span()=> ::nestrs::http::inline_interceptor(#expr))
}

/// Pulls every `#[roles(..)]` / `#[permissions(..)]` attribute, preserving order.
fn take_requirements(attrs: &mut Vec<Attribute>) -> syn::Result<Vec<Requirement>> {
    let mut found = Vec::new();
    let mut failure: Option<syn::Error> = None;
    attrs.retain(|attr| {
        let kind = if attr.path().is_ident("roles") {
            RequirementKind::Roles
        } else if attr.path().is_ident("permissions") {
            RequirementKind::Permissions
        } else {
            return true;
        };
        let parsed = attr
            .parse_args_with(syn::punctuated::Punctuated::<LitStr, syn::Token![,]>::parse_terminated)
            .map_err(|error| {
                syn::Error::new(
                    error.span(),
                    "expected string literals, e.g. `#[roles(\"admin\", \"editor\")]`",
                )
            })
            .and_then(|names| {
                if names.is_empty() {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "list at least one role or permission; an empty requirement would allow nobody",
                    ));
                }
                if let Some(blank) = names.iter().find(|name| name.value().trim().is_empty()) {
                    return Err(syn::Error::new_spanned(
                        blank,
                        "a role or permission name cannot be empty",
                    ));
                }
                Ok(names.iter().map(LitStr::value).collect::<Vec<_>>())
            });
        match parsed {
            Ok(names) => found.push(Requirement {
                kind,
                names,
                span: attr.span(),
            }),
            Err(error) => failure = Some(error),
        }
        false
    });
    match failure {
        Some(error) => Err(error),
        None => Ok(found),
    }
}

/// Pulls every `#[interceptor(..)]` attribute, preserving order.
fn take_interceptors(attrs: &mut Vec<Attribute>) -> syn::Result<Vec<Interceptor>> {
    let mut found = Vec::new();
    let mut failure: Option<syn::Error> = None;
    attrs.retain(|attr| {
        if !attr.path().is_ident("interceptor") {
            return true;
        }
        match Interceptor::parse(attr) {
            Ok(interceptor) => found.push(interceptor),
            Err(error) => failure = Some(error),
        }
        false
    });
    match failure {
        Some(error) => Err(error),
        None => Ok(found),
    }
}

/// Pulls every `#[name(Type)]` attribute (`guard`) out of `attrs`,
/// preserving order.
fn take_typed(attrs: &mut Vec<Attribute>, name: &str) -> syn::Result<Vec<Type>> {
    let mut found = Vec::new();
    let mut failure: Option<syn::Error> = None;

    attrs.retain(|attr| {
        if !attr.path().is_ident(name) {
            return true;
        }
        match attr.parse_args::<Type>() {
            Ok(ty) => found.push(ty),
            Err(error) => failure = Some(error),
        }
        false
    });

    match failure {
        Some(error) => Err(error),
        None => Ok(found),
    }
}

/// For `Inject<T>` / `Piped<P>` returns `T` / `P`: the provider the extractor
/// resolves from the request scope.
fn request_scoped_target(ty: &Type) -> Option<&Type> {
    let Type::Path(path) = ty else { return None };
    let segment = path.path.segments.last()?;
    if segment.ident != "Inject" && segment.ident != "Piped" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    arguments.args.iter().find_map(|argument| match argument {
        syn::GenericArgument::Type(ty) => Some(ty),
        _ => None,
    })
}

fn self_ty_name(ty: &Type) -> String {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default(),
        _ => "Controller".to_owned(),
    }
}

/// The `///` lines of an item.
fn doc_lines(attrs: &[Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .filter_map(|attr| match &attr.meta {
            Meta::NameValue(name_value) => match &name_value.value {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(text),
                    ..
                }) => Some(text.value()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// The single generic argument of the last path segment named `name`.
fn generic_of<'a>(ty: &'a Type, name: &str) -> Option<&'a Type> {
    let Type::Path(path) = ty else { return None };
    let segment = path.path.segments.last()?;
    if segment.ident != name {
        return None;
    }
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    arguments.args.iter().find_map(|argument| match argument {
        syn::GenericArgument::Type(ty) => Some(ty),
        _ => None,
    })
}

/// Finds the `Json<T>` inside a handler's return type (through `Result` and tuples).
fn json_body(ty: &Type) -> Option<&Type> {
    if let Some(inner) = generic_of(ty, "Json") {
        return Some(inner);
    }
    if let Some(ok) = generic_of(ty, "Result") {
        return json_body(ok);
    }
    match ty {
        Type::Tuple(tuple) => tuple.elems.iter().find_map(json_body),
        Type::Paren(paren) => json_body(&paren.elem),
        Type::Group(group) => json_body(&group.elem),
        _ => None,
    }
}

/// Finds the `EventStream<T>` inside a handler's return type (through `Result`).
fn event_stream_body(ty: &Type) -> Option<&Type> {
    if let Some(inner) = generic_of(ty, "EventStream") {
        return Some(inner);
    }
    if let Some(ok) = generic_of(ty, "Result") {
        return event_stream_body(ok);
    }
    match ty {
        Type::Paren(paren) => event_stream_body(&paren.elem),
        Type::Group(group) => event_stream_body(&group.elem),
        _ => None,
    }
}

/// Whether the return type mentions `StatusCode` (so the verb's usual status applies).
fn mentions_status_code(ty: &Type) -> bool {
    match ty {
        Type::Path(path) => {
            let Some(segment) = path.path.segments.last() else {
                return false;
            };
            if segment.ident == "StatusCode" {
                return true;
            }
            generic_of(ty, "Result").is_some_and(mentions_status_code)
        }
        Type::Tuple(tuple) => tuple.elems.iter().any(mentions_status_code),
        Type::Paren(paren) => mentions_status_code(&paren.elem),
        _ => false,
    }
}

/// `{id}` / `{*rest}` names in a route template, in order.
fn template_params(path: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = path;
    while let Some(start) = rest.find('{') {
        let Some(end) = rest[start..].find('}') else { break };
        names.push(rest[start + 1..start + end].trim_start_matches('*').to_owned());
        rest = &rest[start + end + 1..];
    }
    names
}

/// The expression that builds one route's `RouteDoc`.
fn route_doc(
    route: &Route,
    class_requirements: &[Requirement],
    class_guards: &[Type],
    controller: &str,
) -> TokenStream {
    let verb = route.verb;
    let path = &route.path;
    let handler = route.method.to_string();
    let lines = &route.doc_lines;

    let mut path_types: Vec<&Type> = Vec::new();
    let mut query_types: Vec<&Type> = Vec::new();
    let mut pipe_types: Vec<&Type> = Vec::new();
    let mut body_type: Option<&Type> = None;
    let mut validated = false;
    for extractor in &route.extractors {
        if let Some(inner) = generic_of(extractor, "Path") {
            match inner {
                Type::Tuple(tuple) => path_types.extend(tuple.elems.iter()),
                other => path_types.push(other),
            }
        } else if let Some(inner) = generic_of(extractor, "Query") {
            query_types.push(inner);
        } else if let Some(inner) = generic_of(extractor, "Piped") {
            pipe_types.push(inner);
        } else if let Some(inner) = generic_of(extractor, "ValidatedJson") {
            body_type = Some(inner);
            validated = true;
        } else if let Some(inner) = generic_of(extractor, "Json") {
            body_type = Some(inner);
        }
    }

    let path_params = template_params(path).into_iter().zip(path_types).map(|(name, ty)| {
        if last_segment_is(ty, "Uuid") {
            // utoipa describes `Uuid` only inside derived schemas, not as a bare type.
            quote!(.path_param(#name, ::nestrs::http::openapi::uuid_schema()))
        } else {
            quote!(.path_param(#name, (&Probe::<#ty>::new()).path_schema()))
        }
    });
    let queries = query_types
        .iter()
        .map(|ty| quote!(.query((&Probe::<#ty>::new()).query_params())));
    let pipe_params = pipe_types
        .iter()
        .map(|ty| quote!(.query(<#ty as ::nestrs::http::Pipe>::parameters())));
    let request_body = body_type.map(|ty| {
        let body = schema_body(ty);
        quote!(.request_body(#body))
    });

    let output = match &route.output {
        syn::ReturnType::Type(_, ty) => Some(&**ty),
        syn::ReturnType::Default => None,
    };
    let events = output.and_then(event_stream_body);
    let response_body = output.and_then(json_body).or(events);
    let status: u16 = if let Some(declared) = route.status {
        declared
    } else if output.is_some_and(mentions_status_code) {
        match (verb, response_body.is_some()) {
            ("post", _) => 201,
            ("delete", false) => 204,
            _ => 200,
        }
    } else {
        200
    };
    let response_body = match response_body {
        Some(ty) => schema_body(ty),
        None => quote!(::std::option::Option::None),
    };
    let response_ctor = if events.is_some() {
        quote!(ResponseDoc::event_stream)
    } else {
        quote!(ResponseDoc::new)
    };
    let secured = !class_guards.is_empty() || !route.guards.is_empty();
    // Anonymous callers are fine only when every guard on the route is an optional one.
    let guard_types = class_guards.iter().chain(route.guards.iter());
    let anonymous_ok = quote!(true #(&& (&Probe::<#guard_types>::new()).guard_anonymous_ok())*);
    let requires = class_requirements
        .iter()
        .chain(route.requirements.iter())
        .map(|requirement| requirement.describe())
        .map(|text| quote!(.requires(#text)));
    let declared_errors = route
        .responses
        .iter()
        .map(|(code, text)| quote!(.declared_error(#code, #text)));

    quote! {
        {
            use ::nestrs::http::openapi::{
                Probe, ResponseDoc, RouteDoc, ViaGuard as _, ViaIntoParams as _, ViaPartialSchema as _,
                ViaToSchema as _, WithoutGuard as _, WithoutIntoParams as _, WithoutPartialSchema as _,
                WithoutToSchema as _,
            };
            RouteDoc::new(
                #verb,
                ::nestrs::http::join_path(<Self as ::nestrs::http::HttpController>::PREFIX, #path),
                #handler,
                #controller,
            )
            .docs(&[#(#lines),*])
            #(#path_params)*
            #(#queries)*
            #(#pipe_params)*
            #request_body
            .response(#response_ctor(#status, #response_body))
            #(#declared_errors)*
            .secured(#secured)
            .anonymous_ok(#anonymous_ok)
            #(#requires)*
            .validated(#validated)
        }
    }
}

fn last_segment_is(ty: &Type, name: &str) -> bool {
    matches!(ty, Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == name))
}

/// The expression documenting a JSON body of type `ty`; `Vec<T>` becomes an array of `T`.
fn schema_body(ty: &Type) -> TokenStream {
    match generic_of(ty, "Vec") {
        Some(item) => quote!((&Probe::<#item>::new()).schema_body().map(|body| body.into_array())),
        None => quote!((&Probe::<#ty>::new()).schema_body()),
    }
}

/// Pulls `#[status(201)]` out of `attrs`: the default success status.
fn take_status(attrs: &mut Vec<Attribute>) -> syn::Result<Option<(u16, proc_macro2::Span)>> {
    let mut found: Option<(u16, proc_macro2::Span)> = None;
    let mut failure: Option<syn::Error> = None;
    attrs.retain(|attr| {
        if !attr.path().is_ident("status") {
            return true;
        }
        if found.is_some() {
            failure = Some(syn::Error::new_spanned(attr, "a handler can have only one #[status]"));
            return false;
        }
        let parsed = attr.parse_args::<syn::LitInt>().and_then(|lit| {
            let code = lit.base10_parse::<u16>()?;
            if (200..300).contains(&code) {
                Ok((code, lit.span()))
            } else {
                Err(syn::Error::new(
                    lit.span(),
                    "#[status] sets the success status and must be 2xx; document errors with #[response(404, \"..\")]",
                ))
            }
        });
        match parsed {
            Ok(value) => found = Some(value),
            Err(error) => failure = Some(error),
        }
        false
    });
    match failure {
        Some(error) => Err(error),
        None => Ok(found),
    }
}

/// Pulls every `#[response(404, "Not found")]` out of `attrs`.
fn take_responses(attrs: &mut Vec<Attribute>) -> syn::Result<Vec<(u16, String, proc_macro2::Span)>> {
    let mut found = Vec::new();
    let mut failure: Option<syn::Error> = None;
    attrs.retain(|attr| {
        if !attr.path().is_ident("response") {
            return true;
        }
        let parsed = attr.parse_args_with(|input: syn::parse::ParseStream| {
            let code: syn::LitInt = input.parse()?;
            let status = code.base10_parse::<u16>()?;
            if !(400..600).contains(&status) {
                return Err(syn::Error::new(
                    code.span(),
                    "#[response] documents error responses and must be 4xx or 5xx; use #[status] for the success status",
                ));
            }
            input.parse::<syn::Token![,]>()?;
            let text: LitStr = input.parse()?;
            Ok((status, text.value(), code.span()))
        });
        match parsed {
            Ok(value) => found.push(value),
            Err(error) => failure = Some(error),
        }
        false
    });
    match failure {
        Some(error) => Err(error),
        None => Ok(found),
    }
}
