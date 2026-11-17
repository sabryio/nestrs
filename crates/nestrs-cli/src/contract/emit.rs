//! Printing the [`Api`] as a TypeScript oRPC contract (zod schemas + `oc` procedures).
//!
//! Pure text in, text out. Every method is deterministic so the output can be compared with the
//! file on disk (`--check`).

use std::fmt::Write as _;

use super::model::*;

/// oRPC's error code for an HTTP status (`COMMON_ERROR_STATUS_MAP` in `@orpc/client`).
pub fn error_code(status: u16) -> Option<&'static str> {
    Some(match status {
        400 => "BAD_REQUEST",
        401 => "UNAUTHORIZED",
        402 => "PAYMENT_REQUIRED",
        403 => "FORBIDDEN",
        404 => "NOT_FOUND",
        405 => "METHOD_NOT_SUPPORTED",
        406 => "NOT_ACCEPTABLE",
        408 => "TIMEOUT",
        409 => "CONFLICT",
        410 => "GONE",
        411 => "LENGTH_REQUIRED",
        412 => "PRECONDITION_FAILED",
        413 => "PAYLOAD_TOO_LARGE",
        415 => "UNSUPPORTED_MEDIA_TYPE",
        422 => "UNPROCESSABLE_CONTENT",
        428 => "PRECONDITION_REQUIRED",
        429 => "TOO_MANY_REQUESTS",
        499 => "CLIENT_CLOSED_REQUEST",
        500 => "INTERNAL_SERVER_ERROR",
        501 => "NOT_IMPLEMENTED",
        502 => "BAD_GATEWAY",
        503 => "SERVICE_UNAVAILABLE",
        504 => "GATEWAY_TIMEOUT",
        _ => return None,
    })
}

/// A TypeScript string literal.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// An object key: bare when it is an identifier.
fn key(name: &str) -> String {
    let mut chars = name.chars();
    let identifier = chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_' || first == '$')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$');
    if identifier { name.to_owned() } else { quote(name) }
}

/// One line of prose: line breaks and runs of spaces collapse to one space.
fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn comment(text: &str) -> String {
    format!("/** {} */\n", flat(text).replace("*/", "*\\/"))
}

fn number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

/// zod source for `ty` with the validation `rules` that apply to it.
fn type_expr(ty: &TypeRef, rules: &[Rule]) -> String {
    let bounds = |mut base: String, min_name: &str| {
        for rule in rules {
            match rule {
                Rule::Length { min, max } => {
                    if let Some(min) = min {
                        let _ = write!(base, ".{min_name}({min})");
                    }
                    if let Some(max) = max {
                        let _ = write!(base, ".max({max})");
                    }
                }
                Rule::Range { min, max } => {
                    if let Some(min) = min {
                        let _ = write!(base, ".min({})", number(*min));
                    }
                    if let Some(max) = max {
                        let _ = write!(base, ".max({})", number(*max));
                    }
                }
                _ => {}
            }
        }
        base
    };
    match ty {
        TypeRef::String => {
            let base = if rules.contains(&Rule::Email) {
                "z.email()"
            } else if rules.contains(&Rule::Url) {
                "z.url()"
            } else {
                "z.string()"
            };
            bounds(base.to_owned(), "min")
        }
        TypeRef::Int { unsigned } => {
            let has_lower = rules
                .iter()
                .any(|rule| matches!(rule, Rule::Range { min: Some(_), .. }));
            bounds(
                format!(
                    "z.number().int(){}",
                    if *unsigned && !has_lower { ".min(0)" } else { "" }
                ),
                "min",
            )
        }
        TypeRef::Float => bounds("z.number()".to_owned(), "min"),
        TypeRef::Bool => "z.boolean()".to_owned(),
        TypeRef::Uuid => "z.uuid()".to_owned(),
        TypeRef::DateTime => "z.iso.datetime()".to_owned(),
        TypeRef::Any => "z.unknown()".to_owned(),
        TypeRef::Named(name) => name.clone(),
        TypeRef::Option(inner) => format!("{}.nullable()", type_expr(inner, rules)),
        TypeRef::List(inner) => bounds(format!("z.array({})", type_expr(inner, &[])), "min"),
        TypeRef::Map(inner) => format!("z.record(z.string(), {})", type_expr(inner, &[])),
    }
}

fn field_line(field: &Field, indent: &str) -> String {
    let mut expr = type_expr(&field.ty, &field.rules);
    if let Some(docs) = &field.docs {
        let _ = write!(expr, ".describe({})", quote(&flat(docs)));
    }
    if field.optional {
        expr.push_str(".optional()");
    }
    format!("{indent}{}: {expr},\n", key(&field.name))
}

fn object(fields: &[Field], extra: Option<String>, indent: &str) -> String {
    let inner = format!("{indent}  ");
    let mut out = String::from("z.object({\n");
    for field in fields {
        out.push_str(&field_line(field, &inner));
    }
    if let Some(extra) = extra {
        let _ = writeln!(out, "{inner}{extra},");
    }
    let _ = write!(out, "{indent}}})");
    out
}

fn schema(schema: &Schema) -> String {
    let mut out = String::new();
    if let Some(docs) = &schema.docs {
        out.push_str(&comment(docs));
    }
    let expr = match &schema.kind {
        SchemaKind::Object(fields) => object(fields, None, ""),
        SchemaKind::Alias(inner) => type_expr(inner, &[]),
        SchemaKind::Enum(names) => format!(
            "z.enum([{}])",
            names.iter().map(|name| quote(name)).collect::<Vec<_>>().join(", ")
        ),
        SchemaKind::Tagged { tag, variants } => {
            let mut text = format!("z.discriminatedUnion({}, [\n", quote(tag));
            for variant in variants {
                let literal = format!("{}: z.literal({})", key(tag), quote(&variant.tag));
                let mut member = object(&variant.fields, Some(literal), "  ");
                if let Some(docs) = &variant.docs {
                    let _ = write!(member, ".describe({})", quote(&flat(docs)));
                }
                let _ = writeln!(text, "  {member},");
            }
            text.push_str("])");
            text
        }
    };
    let name = &schema.name;
    let _ = write!(
        out,
        "export const {name} = {expr};\nexport type {name} = z.infer<typeof {name}>;\n"
    );
    out
}

fn input(route: &Route, schemas: &[Schema]) -> Option<String> {
    let input = &route.input;
    let mut extras: Vec<Field> = input
        .path
        .iter()
        .map(|(name, ty)| Field {
            name: name.clone(),
            docs: None,
            ty: ty.clone(),
            optional: false,
            rules: Vec::new(),
        })
        .collect();
    extras.extend(input.query.iter().cloned());

    match (&input.body, extras.is_empty()) {
        (None, true) => None,
        (None, false) => Some(object(&extras, None, "      ")),
        (Some(body), true) => Some(type_expr(body, &[])),
        (Some(body), false) => {
            let is_object = matches!(body, TypeRef::Named(name)
                if schemas.iter().any(|s| &s.name == name && matches!(s.kind, SchemaKind::Object(_))));
            let extra = object(&extras, None, "      ");
            Some(if is_object {
                format!("{}.extend({})", type_expr(body, &[]), strip_wrapper(&extra))
            } else {
                format!("z.intersection({}, {extra})", type_expr(body, &[]))
            })
        }
    }
}

/// `z.object({..})` → `{..}` (for `.extend`).
fn strip_wrapper(object: &str) -> &str {
    object
        .strip_prefix("z.object(")
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or(object)
}

fn procedure(route: &Route, controller: &Controller, schemas: &[Schema]) -> String {
    let mut out = format!("    {}: oc\n      .route({{\n", key(&route.name));
    let _ = writeln!(out, "        method: {},", quote(route.method));
    let _ = writeln!(out, "        path: {},", quote(&route.path));
    if let Some(status) = route.success_status {
        let _ = writeln!(out, "        successStatus: {status},");
    }
    if let Some(summary) = &route.summary {
        let _ = writeln!(out, "        summary: {},", quote(summary));
    }
    if let Some(description) = &route.description {
        let _ = writeln!(out, "        description: {},", quote(description));
    }
    let _ = writeln!(out, "        tags: [{}],", quote(&controller.type_name));
    out.push_str("      })");

    if !route.errors.is_empty() {
        out.push_str("\n      .errors({\n");
        for case in &route.errors {
            let code = error_code(case.status).expect("statuses are checked when the route is read");
            let _ = writeln!(
                out,
                "        {code}: {{ message: {}, data: ErrorBody }},",
                quote(&case.message)
            );
        }
        out.push_str("      })");
    }
    if let Some(input) = input(route, schemas) {
        let _ = write!(out, "\n      .input({input})");
    }
    let output = match &route.output {
        Output::Nothing => "z.void()".to_owned(),
        Output::Json(ty) => type_expr(ty, &[]),
        Output::Events(ty) => format!("eventIterator({})", type_expr(ty, &[])),
    };
    let _ = write!(out, "\n      .output({output}),\n");
    out
}

/// The whole file.
pub fn contract(api: &Api) -> String {
    let uses_errors = api
        .controllers
        .iter()
        .flat_map(|c| &c.routes)
        .any(|r| !r.errors.is_empty());
    let uses_events = api
        .controllers
        .iter()
        .flat_map(|c| &c.routes)
        .any(|r| matches!(r.output, Output::Events(_)));

    let mut out = String::from(
        "/* eslint-disable */\n\
         // GENERATED FILE: do not edit.\n\
         // Generated by `nestrs generate contract` from the Rust controllers and DTOs.\n\
         // Regenerate with the same command; `--check` fails when this file is stale.\n\n\
         import \"@orpc/openapi/extensions/route\";\n",
    );
    let _ = writeln!(
        out,
        "import {{ {}oc }} from \"@orpc/contract\";",
        if uses_events { "eventIterator, " } else { "" }
    );
    out.push_str("import { z } from \"zod\";\n\n");
    let _ = writeln!(
        out,
        "/** Where the server mounts the API; pass it as the link's `url`. */\nexport const apiPrefix = {} as `/${{string}}`;\n",
        quote(if api.prefix.is_empty() { "/" } else { &api.prefix })
    );

    out.push_str("// ---------------------------------------------------------------- schemas\n\n");
    if uses_errors {
        out.push_str(
            "/** The `error` body every failure carries (see `HttpError`). */\n\
             export const ErrorBody = z.object({\n  \
             code: z.string().describe(\"Stable machine-readable code (`not_found`, `validation_failed`, ..).\"),\n  \
             details: z.unknown().describe(\"Per-field details for validation failures.\").optional(),\n  \
             message: z.string().describe(\"Human-readable message.\"),\n});\n\
             export type ErrorBody = z.infer<typeof ErrorBody>;\n\n",
        );
    }
    for item in &api.schemas {
        out.push_str(&schema(item));
        out.push('\n');
    }

    out.push_str(
        "// --------------------------------------------------------------- contract\n\nexport const contract = {\n",
    );
    for controller in &api.controllers {
        if controller.routes.is_empty() {
            continue;
        }
        let _ = writeln!(out, "  {}: {{", key(&controller.key));
        for route in &controller.routes {
            out.push_str(&procedure(route, controller, &api.schemas));
        }
        out.push_str("  },\n");
    }
    out.push_str("};\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(name: &str) -> Route {
        Route {
            name: name.into(),
            method: "GET",
            path: "/things/{id}".into(),
            summary: Some("Fetch \"one\".".into()),
            description: None,
            success_status: None,
            errors: vec![ErrorCase {
                status: 404,
                message: "No thing".into(),
            }],
            input: Input {
                path: vec![("id".into(), TypeRef::Uuid)],
                ..Input::default()
            },
            output: Output::Json(TypeRef::Named("Thing".into())),
        }
    }

    #[test]
    fn a_route_becomes_a_procedure_with_typed_errors() {
        let api = Api {
            prefix: "/api".into(),
            controllers: vec![Controller {
                type_name: "ThingsController".into(),
                key: "things".into(),
                routes: vec![route("find")],
            }],
            schemas: vec![Schema {
                name: "Thing".into(),
                docs: Some("A thing.".into()),
                kind: SchemaKind::Object(vec![Field {
                    name: "id".into(),
                    docs: Some("Identifier.".into()),
                    ty: TypeRef::Uuid,
                    optional: false,
                    rules: vec![],
                }]),
            }],
        };
        let text = contract(&api);
        assert!(text.contains("export const apiPrefix = \"/api\" as"), "{text}");
        assert!(
            text.contains("/** A thing. */\nexport const Thing = z.object({"),
            "{text}"
        );
        assert!(text.contains("id: z.uuid().describe(\"Identifier.\"),"), "{text}");
        assert!(text.contains("summary: \"Fetch \\\"one\\\".\","), "{text}");
        assert!(
            text.contains("NOT_FOUND: { message: \"No thing\", data: ErrorBody },"),
            "{text}"
        );
        assert!(
            text.contains(".input(z.object({\n        id: z.uuid(),\n      }))"),
            "{text}"
        );
        assert!(text.contains(".output(Thing),"), "{text}");
        assert!(!text.contains("eventIterator"), "no unused import");
    }

    #[test]
    fn validation_rules_and_optionality_reach_the_schema() {
        let field = Field {
            name: "title".into(),
            docs: None,
            ty: TypeRef::Option(Box::new(TypeRef::String)),
            optional: true,
            rules: vec![Rule::Length {
                min: Some(1),
                max: Some(200),
            }],
        };
        assert_eq!(
            field_line(&field, ""),
            "title: z.string().min(1).max(200).nullable().optional(),\n"
        );
        assert_eq!(type_expr(&TypeRef::String, &[Rule::Email]), "z.email()");
        assert_eq!(
            type_expr(
                &TypeRef::Int { unsigned: true },
                &[Rule::Range {
                    min: None,
                    max: Some(100.0)
                }]
            ),
            "z.number().int().min(0).max(100)"
        );
    }

    #[test]
    fn keys_are_quoted_only_when_they_must_be() {
        assert_eq!(key("author_id"), "author_id");
        assert_eq!(key("x-id"), "\"x-id\"");
    }
}
