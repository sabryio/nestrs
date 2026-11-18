//! `purlin generate contract`: the TypeScript oRPC contract, derived from the Rust controllers.
//!
//! The Rust side already says everything a client needs: routes (`#[get("/{id}")]`), inputs
//! (`Path`, `Query`, `ValidatedJson`), outputs (`Json<T>`, `EventStream<T>`), documented failures
//! (`#[response(404, "..")]`), and the DTOs with their serde and validator attributes. Writing it
//! again in TypeScript is redundant work that drifts. This reads the sources with `syn`, so it
//! needs no compiled binary and no running server.
//!
//! | Module | Job |
//! |---|---|
//! | `scan` | parse the files; find types, controllers and the mount prefix |
//! | `lower` | resolve types and signatures into the [`model`] (refusing what JSON cannot express) |
//! | `emit` | print the model as TypeScript |
//!
//! What cannot be expressed is reported as a warning and that route is left out, so the contract
//! never lies; `--strict` turns warnings into a failing exit for CI.

mod emit;
mod lower;
pub mod model;
mod scan;
mod serde_names;

use std::path::{Path, PathBuf};

use crate::workspace::Workspace;

/// Generation failed.
#[derive(Debug, thiserror::Error)]
pub enum ContractError {
    /// A source file is not valid Rust.
    #[error("cannot parse {path}: {message}")]
    Parse {
        /// The file.
        path: String,
        /// What `syn` said.
        message: String,
    },
    /// No `#[controller]` was found, so there is nothing to describe.
    #[error("no #[controller] found under src/: run this in a purlin project")]
    NoControllers,
    /// `--check`: the file on disk differs from what the sources produce.
    #[error("{path} is out of date; run `purlin generate contract`")]
    Stale {
        /// The contract file.
        path: String,
        /// What would change.
        diff: String,
    },
    /// `--strict` and there were warnings.
    #[error("{} warning(s) in strict mode", .0.len())]
    Strict(Vec<String>),
}

/// How the command runs.
#[derive(Debug, Clone, Default)]
pub struct ContractOptions {
    /// Where the contract is written, relative to the project root.
    pub out: PathBuf,
    /// More source directories to read controllers and DTOs from, relative to the project root
    /// (a controller shipped by a library crate, such as `purlin-health`'s).
    pub also: Vec<PathBuf>,
    /// Overrides the mount prefix otherwise read from `.prefix("..")` in `main.rs`.
    pub prefix: Option<String>,
    /// Compare instead of write; fails when the file is stale.
    pub check: bool,
    /// Fail on warnings.
    pub strict: bool,
    /// Show the change without writing.
    pub dry_run: bool,
}

/// What a run found and did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContractReport {
    /// Routes in the contract.
    pub routes: usize,
    /// Named schemas in the contract.
    pub schemas: usize,
    /// Things left out, each with the reason.
    pub warnings: Vec<String>,
    /// The contract file, as a path string.
    pub path: String,
    /// Whether the file's contents changed (or would, on a dry run).
    pub changed: bool,
    /// Unified diff of the change; empty when nothing changed.
    pub diff: String,
}

/// Builds the contract text from source files. Pure: no workspace, no disk.
pub fn build(files: &[(PathBuf, String)], prefix: Option<&str>) -> Result<(String, ContractReport), ContractError> {
    let sources = scan::scan(files)?;
    if sources.controllers.is_empty() {
        return Err(ContractError::NoControllers);
    }
    let prefix = prefix
        .map(str::to_owned)
        .or_else(|| sources.prefix.clone())
        .unwrap_or_default();

    let mut lowerer = lower::Lowerer::new(&sources);
    let mut controllers: Vec<_> = sources
        .controllers
        .iter()
        .filter_map(|item| lowerer.controller(item))
        .collect();
    controllers.sort_by(|a, b| a.key.cmp(&b.key));
    let (schemas, warnings) = lowerer.finish();

    let api = model::Api {
        prefix,
        controllers,
        schemas,
    };
    let report = ContractReport {
        routes: api.controllers.iter().map(|c| c.routes.len()).sum(),
        schemas: api.schemas.len(),
        warnings,
        ..ContractReport::default()
    };
    Ok((emit::contract(&api), report))
}

/// The command: reads `src/**/*.rs` through `workspace`, writes (or checks) `options.out`.
pub fn run(workspace: &mut dyn Workspace, options: &ContractOptions) -> Result<ContractReport, crate::CliError> {
    let mut files = Vec::new();
    for dir in std::iter::once(Path::new("src")).chain(options.also.iter().map(PathBuf::as_path)) {
        for path in workspace.rust_files(dir)? {
            if let Some(text) = workspace.read(&path)? {
                files.push((path, text));
            }
        }
    }
    let (text, mut report) = build(&files, options.prefix.as_deref())?;

    let existing = workspace.read(&options.out)?;
    let path = options.out.display().to_string();
    report.diff = crate::diff::unified(&path, existing.as_deref(), &text);
    report.changed = existing.as_deref() != Some(text.as_str());
    report.path = path.clone();

    if options.strict && !report.warnings.is_empty() {
        return Err(ContractError::Strict(report.warnings).into());
    }
    if options.check {
        if report.changed {
            return Err(ContractError::Stale {
                path,
                diff: report.diff,
            }
            .into());
        }
        return Ok(report);
    }
    if report.changed && !options.dry_run {
        workspace.write(&options.out, &text)?;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::MemoryWorkspace;

    const SOURCE: &str = r#"
        /// A thing.
        #[derive(Serialize, Deserialize, Validate)]
        #[serde(rename_all = "camelCase")]
        pub struct Thing {
            /// Identifier.
            pub id: Uuid,
            #[validate(length(min = 1, max = 50))]
            pub display_name: String,
            pub note: Option<String>,
            #[serde(default)]
            pub tags: Vec<String>,
            #[serde(skip)]
            pub secret: String,
            #[serde(rename = "when")]
            pub created_at: DateTime<Utc>,
            #[validate(range(min = 0, max = 10))]
            pub score: u8,
            pub extra: HashMap<String, i64>,
            pub kind: Kind,
        }

        #[derive(Serialize)]
        #[serde(rename_all = "snake_case")]
        pub enum Kind { FirstKind, SecondKind }

        #[derive(Serialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        pub enum Event {
            /// Made.
            Made { thing: Thing },
            Gone,
        }

        #[derive(Deserialize)]
        pub struct Search { pub q: String, pub limit: Option<u32> }

        pub struct Wrapper(Uuid);

        #[controller("/things")]
        #[guard(BearerAuth)]
        impl ThingsController {
            /// List things.
            ///
            /// Filtered by `q`.
            #[get]
            async fn list(&self, Query(search): Query<Search>) -> Result<Json<Vec<Thing>>, HttpError> { todo!() }

            #[get("/{id}")]
            #[response(404, "No thing")]
            async fn find(&self, Path(id): Path<Uuid>) -> Result<Json<Thing>, HttpError> { todo!() }

            #[put("/{shelf}/items/{item}")]
            #[roles("admin", "editor")]
            #[status(201)]
            async fn put_item(&self, Path((shelf, item)): Path<(Uuid, i64)>, ValidatedJson(thing): ValidatedJson<Thing>) -> Result<Json<Thing>, HttpError> { todo!() }

            #[delete("/{id}")]
            #[status(204)]
            async fn remove(&self, Path(id): Path<Uuid>) -> Result<(), HttpError> { todo!() }

            #[get("/events")]
            async fn events(&self, last: LastEventId) -> EventStream<Event> { todo!() }

            #[get("/odd")]
            async fn odd(&self) -> impl IntoResponse { todo!() }
        }
    "#;

    fn build_one(source: &str) -> (String, ContractReport) {
        build(&[(PathBuf::from("src/lib.rs"), source.to_owned())], Some("/api")).unwrap()
    }

    #[test]
    fn types_follow_serde_and_validator_attributes() {
        let (text, _) = build_one(SOURCE);
        assert!(
            text.contains("displayName: z.string().min(1).max(50),"),
            "camelCase + length:\n{text}"
        );
        assert!(text.contains("note: z.string().nullable().optional(),"), "{text}");
        assert!(
            text.contains("tags: z.array(z.string()).optional(),"),
            "default makes it optional:\n{text}"
        );
        assert!(!text.contains("secret"), "skipped fields stay off the wire");
        assert!(text.contains("when: z.iso.datetime(),"), "rename wins:\n{text}");
        assert!(text.contains("score: z.number().int().min(0).max(10),"), "{text}");
        assert!(
            text.contains("extra: z.record(z.string(), z.number().int()),"),
            "{text}"
        );
        assert!(text.contains("kind: Kind,"), "{text}");
        assert!(
            text.contains("export const Kind = z.enum([\"first_kind\", \"second_kind\"]);"),
            "{text}"
        );
        assert!(text.contains("z.discriminatedUnion(\"type\", ["), "{text}");
        assert!(
            text.contains("type: z.literal(\"made\")") && text.contains("type: z.literal(\"gone\")"),
            "{text}"
        );
        let kind = text.find("export const Kind").unwrap();
        let thing = text.find("export const Thing").unwrap();
        assert!(kind < thing, "dependencies are declared first");
    }

    #[test]
    fn routes_carry_paths_inputs_outputs_and_errors() {
        let (text, report) = build_one(SOURCE);
        assert_eq!(report.routes, 5);
        assert!(text.contains("path: \"/things\","), "{text}");
        assert!(text.contains("path: \"/things/{shelf}/items/{item}\","), "{text}");
        assert!(
            text.contains("method: \"PUT\"") && text.contains("successStatus: 201,"),
            "{text}"
        );
        assert!(
            text.contains("successStatus: 204,") && text.contains(".output(z.void())"),
            "{text}"
        );
        assert!(text.contains("summary: \"List things.\","), "{text}");
        assert!(text.contains("description: \"Filtered by `q`.\","), "{text}");
        assert!(text.contains("Requires role admin or editor."), "{text}");
        assert!(text.contains(".output(eventIterator(Event))"), "{text}");
        assert!(
            text.contains("NOT_FOUND: { message: \"No thing\", data: ErrorBody },"),
            "{text}"
        );
        assert!(
            text.contains("UNAUTHORIZED:") && text.contains("FORBIDDEN:") && text.contains("UNPROCESSABLE_CONTENT:"),
            "{text}"
        );
        // Path params and a body are merged into one input object.
        assert!(text.contains("Thing.extend({"), "{text}");
        assert!(
            text.contains("shelf: z.uuid(),") && text.contains("item: z.number().int(),"),
            "{text}"
        );
        // Query struct fields become input fields.
        assert!(
            text.contains("q: z.string(),") && text.contains("limit: z.number().int().min(0).nullable().optional(),"),
            "{text}"
        );
    }

    #[test]
    fn what_cannot_be_expressed_is_reported_and_left_out() {
        let (text, report) = build_one(SOURCE);
        assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
        assert!(report.warnings[0].contains("ThingsController::odd") && report.warnings[0].contains("impl Trait"));
        assert!(!text.contains("odd:"));
    }

    #[test]
    fn unsupported_types_name_the_type_and_the_field() {
        let source = r#"
            pub struct Bad { pub when: Decimal }
            #[controller("/bad")]
            impl BadController {
                #[get] async fn one(&self) -> Json<Bad> { todo!() }
            }"#;
        let (_, report) = build_one(source);
        assert!(
            report.warnings[0].contains("Bad: field `when`") && report.warnings[0].contains("Decimal"),
            "{:?}",
            report.warnings
        );
        assert_eq!(report.routes, 0);
    }

    #[test]
    fn recursive_and_untagged_types_are_refused_not_looped_on() {
        let source = r#"
            pub struct Tree { pub child: Option<Box<Tree>> }
            #[derive(Serialize)] #[serde(untagged)] pub enum U { A(i32) }
            #[controller("/t")]
            impl TController {
                #[get] async fn tree(&self) -> Json<Tree> { todo!() }
                #[get("/u")] async fn u(&self) -> Json<U> { todo!() }
            }"#;
        let (_, report) = build_one(source);
        assert!(
            report.warnings.iter().any(|w| w.contains("recursive")),
            "{:?}",
            report.warnings
        );
        assert!(
            report.warnings.iter().any(|w| w.contains("untagged")),
            "{:?}",
            report.warnings
        );
    }

    #[test]
    fn newtypes_are_transparent() {
        let source = r#"
            pub struct Id(Uuid);
            pub struct Holder { pub id: Id }
            #[controller("/h")]
            impl HController { #[get] async fn one(&self) -> Json<Holder> { todo!() } }"#;
        let (text, _) = build_one(source);
        assert!(text.contains("export const Id = z.uuid();"), "{text}");
        assert!(text.contains("id: Id,"), "{text}");
    }

    #[test]
    fn tuple_returns_use_the_json_part() {
        let source = r#"
            pub struct R { pub a: i32 }
            #[controller("/r")]
            impl RController {
                #[post] async fn make(&self) -> Result<([(&'static str, String); 1], Json<R>), HttpError> { todo!() }
            }"#;
        let (text, report) = build_one(source);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert!(text.contains(".output(R),"), "{text}");
    }

    #[test]
    fn no_controllers_is_an_error_not_an_empty_contract() {
        let error = build(&[(PathBuf::from("src/lib.rs"), "struct A;".into())], None).unwrap_err();
        assert!(matches!(error, ContractError::NoControllers));
    }

    #[test]
    fn output_is_deterministic() {
        assert_eq!(build_one(SOURCE).0, build_one(SOURCE).0);
    }

    fn project() -> MemoryWorkspace {
        MemoryWorkspace::new().with("src/lib.rs", SOURCE)
    }

    fn options() -> ContractOptions {
        ContractOptions {
            out: "web/contract.ts".into(),
            prefix: Some("/api".into()),
            ..ContractOptions::default()
        }
    }

    #[test]
    fn writes_then_reports_unchanged_then_check_detects_drift() {
        let mut ws = project();
        let first = run(&mut ws, &options()).unwrap();
        assert!(first.changed && ws.get("web/contract.ts").is_some());

        let second = run(&mut ws, &options()).unwrap();
        assert!(!second.changed && second.diff.is_empty());

        let check = ContractOptions {
            check: true,
            ..options()
        };
        run(&mut ws, &check).unwrap();

        ws.write(Path::new("src/lib.rs"), &SOURCE.replace("/things", "/stuff"))
            .unwrap();
        match run(&mut ws, &check).unwrap_err() {
            crate::CliError::Contract(ContractError::Stale { diff, .. }) => {
                assert!(
                    diff.contains("-        path: \"/things\"") && diff.contains("+        path: \"/stuff\""),
                    "{diff}"
                );
            }
            other => panic!("{other}"),
        }
        assert!(
            ws.get("web/contract.ts").unwrap().contains("/things"),
            "--check never writes"
        );
    }

    #[test]
    fn a_dry_run_shows_the_diff_and_writes_nothing() {
        let mut ws = project();
        let report = run(
            &mut ws,
            &ContractOptions {
                dry_run: true,
                ..options()
            },
        )
        .unwrap();
        assert!(report.changed && report.diff.contains("+export const contract"));
        assert!(ws.get("web/contract.ts").is_none());
    }

    #[test]
    fn strict_mode_fails_on_warnings() {
        let mut ws = project();
        let error = run(
            &mut ws,
            &ContractOptions {
                strict: true,
                ..options()
            },
        )
        .unwrap_err();
        assert!(matches!(error, crate::CliError::Contract(ContractError::Strict(w)) if w.len() == 1));
        assert!(ws.get("web/contract.ts").is_none(), "nothing is written on failure");
    }

    #[test]
    fn extra_source_directories_are_read() {
        let mut ws = MemoryWorkspace::new().with("src/lib.rs", "struct A;").with(
            "vendor/health/src/lib.rs",
            "#[controller(\"/health\")] impl HealthController { #[get(\"/live\")] async fn live(&self) {} }",
        );
        let report = run(
            &mut ws,
            &ContractOptions {
                also: vec!["vendor/health/src".into()],
                ..options()
            },
        )
        .unwrap();
        assert_eq!(report.routes, 1);
    }
}
