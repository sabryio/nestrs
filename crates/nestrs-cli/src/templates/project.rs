//! `nestrs new`: an empty clean-architecture service that runs and has a passing test.

use super::fill;
use crate::plan::Plan;

/// Where the generated project gets nestrs from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencySource {
    /// Published crates, at this version requirement (`"0.1"`).
    Version(String),
    /// A local checkout of the nestrs workspace (for development before publishing).
    Path(String),
}

impl DependencySource {
    fn dep(&self, crate_name: &str) -> String {
        match self {
            Self::Version(version) => format!("{crate_name} = \"{version}\""),
            Self::Path(root) => format!(
                "{crate_name} = {{ path = \"{}/crates/{crate_name}\" }}",
                root.trim_end_matches('/')
            ),
        }
    }
}

const CARGO: &str = r#"[package]
name = "__PACKAGE__"
version = "0.1.0"
edition = "2024"
publish = false

[dependencies]
__NESTRS__
__KIT__
async-trait = "0.1"
serde = { version = "1", features = ["derive"] }
thiserror = "2"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
tracing = "0.1"
utoipa = "5"
uuid = { version = "1", features = ["serde", "v4"] }
validator = { version = "0.21", features = ["derive"] }

[dev-dependencies]
__TESTING__
tokio = { version = "1", features = ["macros", "rt-multi-thread", "test-util"] }
serde_json = "1"
"#;

const LIB: &str = r#"//! __PACKAGE__, in clean architecture.
//!
//! ```text
//! domain/          entities, value objects, repository ports        (depends on nothing)
//! application/     use cases                                        (-> domain)
//! infrastructure/  adapters: persistence, config, clients           (-> domain)
//! presentation/    HTTP controllers, DTOs, error mapping            (-> application, domain)
//! composition/     nestrs modules wiring ports to adapters          (-> everything)
//! ```
//!
//! Dependencies point inward only. [`AppModule`] is the composition root.

pub mod application;
pub mod composition;
pub mod domain;
pub mod infrastructure;
pub mod presentation;

pub use composition::AppModule;
"#;

const MAIN: &str = r#"//! Entry point: logging, `/api` prefix, OpenAPI (`--print-openapi`), graceful shutdown.

use nestrs::prelude::*;
use nestrs_kit::Service;

#[tokio::main]
async fn main() -> Result<(), AppError> {
    Service::new("__PACKAGE__", env!("CARGO_PKG_VERSION"))
        .prefix("/api")
        .run(__LIB__::AppModule, |_| Ok("0.0.0.0:3000".to_owned()))
        .await
}
"#;

const APP: &str = r#"use nestrs::prelude::*;
use nestrs_kit::KitModule;

/// The composition root: feature modules are imported here (`nestrs generate feature`
/// adds them for you).
#[module(imports = [HttpModule::module(), KitModule])]
pub struct AppModule;
"#;

const SMOKE_TEST: &str = r#"//! The app boots and answers; feature tests live next to their feature.

use __LIB__::AppModule;
use nestrs_testing::TestApp;

#[tokio::test]
async fn the_app_starts_and_shuts_down_cleanly() {
    let app = TestApp::start(AppModule).await;
    assert!(app.shutdown().await.is_empty());
}
"#;

const GITIGNORE: &str = "/target\n";

/// The plan for a new project named `package` (library name `lib`).
pub fn project(package: &str, lib: &str, source: &DependencySource) -> Plan {
    let tokens = [
        ("__PACKAGE__", package),
        ("__LIB__", lib),
        ("__NESTRS__", &source.dep("nestrs")),
        ("__KIT__", &source.dep("nestrs-kit")),
        ("__TESTING__", &source.dep("nestrs-testing")),
    ];
    let mut plan = Plan::new();
    plan.create("Cargo.toml", fill(CARGO, &tokens))
        .create(".gitignore", GITIGNORE)
        .create("src/lib.rs", fill(LIB, &tokens))
        .create("src/main.rs", fill(MAIN, &tokens))
        .create("src/domain/mod.rs", "//! **Domain layer**: entities, value objects and the ports the domain needs.\n")
        .create("src/application/mod.rs", "//! **Application layer**: use cases. Depends on the domain only.\n")
        .create(
            "src/infrastructure/mod.rs",
            "//! **Infrastructure layer**: adapters that implement the domain's ports.\n",
        )
        .create("src/presentation/mod.rs", "//! **Presentation layer**: how the outside world reaches the use cases.\n\npub mod http;\n")
        .create("src/presentation/http/mod.rs", "//! HTTP controllers, DTOs and the mapping from failures to status codes.\n")
        .create("src/composition/mod.rs", "//! **Composition layer**: nestrs modules wiring ports to adapters.\n\nmod app;\n\npub use app::AppModule;\n")
        .create("src/composition/app.rs", APP)
        .create("tests/smoke.rs", fill(SMOKE_TEST, &tokens));
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_source_points_every_nestrs_crate_at_the_checkout() {
        let plan = project("my-api", "my_api", &DependencySource::Path("/src/nestrs/".into()));
        let cargo = plan
            .ops
            .iter()
            .find_map(|op| match op {
                crate::Op::Create { path, contents } if path.ends_with("Cargo.toml") => Some(contents.clone()),
                _ => None,
            })
            .unwrap();
        assert!(
            cargo.contains("nestrs = { path = \"/src/nestrs/crates/nestrs\" }"),
            "{cargo}"
        );
        assert!(cargo.contains("nestrs-testing = { path = \"/src/nestrs/crates/nestrs-testing\" }"));
        assert!(!cargo.contains("__"), "every token was replaced:\n{cargo}");
    }
}
