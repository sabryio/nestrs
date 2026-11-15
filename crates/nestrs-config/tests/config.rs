//! Layering, parsing, validation and the module for `nestrs-config`.

use std::io::Write;
use std::sync::Arc;

use nestrs_config::{ConfigError, ConfigModule, ConfigSource};
use nestrs_core::{DiError, Module, ModuleDef, ModuleRef, bootstrap};
use serde::Deserialize;
use validator::Validate;

#[derive(Debug, Clone, Deserialize, Validate)]
#[serde(default)]
struct Settings {
    #[validate(length(min = 4, message = "must have at least 4 characters"))]
    token: String,
    #[validate(range(min = 1, message = "must be at least 1"))]
    port: u16,
    verbose: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            token: "default-token".to_owned(),
            port: 3000,
            verbose: false,
        }
    }
}

#[test]
fn type_defaults_apply_when_nothing_else_is_configured() {
    let settings: Settings = ConfigSource::new().load().unwrap();
    assert_eq!(
        (settings.token.as_str(), settings.port, settings.verbose),
        ("default-token", 3000, false)
    );
}

#[test]
fn files_override_defaults_and_overrides_win_over_files() {
    let mut file = tempfile_toml("token = \"from-file\"\nport = 8080\n");
    let path = file.path();
    let settings: Settings = ConfigSource::new().file(&path).load().unwrap();
    assert_eq!((settings.token.as_str(), settings.port), ("from-file", 8080));

    let settings: Settings = ConfigSource::new().file(&path).set("port", 9000).load().unwrap();
    assert_eq!(settings.port, 9000);
    file.close();
}

#[test]
fn a_missing_optional_file_is_skipped_but_a_required_one_is_not() {
    assert!(
        ConfigSource::new()
            .file("/definitely/not/here.toml")
            .load::<Settings>()
            .is_ok()
    );
    let error = ConfigSource::new()
        .required_file("/definitely/not/here.toml")
        .load::<Settings>()
        .unwrap_err();
    assert!(matches!(error, ConfigError::Load(_)), "{error}");
}

#[test]
fn environment_variables_are_parsed_to_the_field_types() {
    // Injected variables: editing the real environment is `unsafe` in edition 2024.
    let settings: Settings = ConfigSource::new()
        .env_prefix("NESTRSCFGA")
        .env_vars([
            ("NESTRSCFGA_PORT", "4242"),
            ("NESTRSCFGA_VERBOSE", "true"),
            ("NESTRSCFGA_TOKEN", "from-env"),
        ])
        .load()
        .unwrap();
    assert_eq!(
        (settings.token.as_str(), settings.port, settings.verbose),
        ("from-env", 4242, true)
    );
}

#[test]
fn every_validation_problem_is_reported_at_once() {
    let error = ConfigSource::new()
        .set("token", "abc")
        .set("port", 0)
        .load::<Settings>()
        .unwrap_err();
    match error {
        ConfigError::Invalid(problems) => {
            assert_eq!(problems.len(), 2, "{problems:?}");
            assert!(
                problems[0].contains("port") && problems[1].contains("token"),
                "{problems:?}"
            );
        }
        other => panic!("expected a validation error, got {other}"),
    }
}

#[test]
fn values_of_the_wrong_type_are_a_load_error() {
    let error = ConfigSource::new()
        .set("port", "not-a-number")
        .load::<Settings>()
        .unwrap_err();
    assert!(matches!(error, ConfigError::Load(_)), "{error}");
}

fn configured(source: ConfigSource) -> ModuleRef {
    ModuleDef::new("AppModule")
        .imports([ConfigModule::for_root::<Settings>(source)])
        .into_ref()
}

#[tokio::test]
async fn the_config_module_provides_the_typed_value_globally() {
    struct Root;
    impl Module for Root {
        fn definition() -> ModuleDef {
            ModuleDef::new("Root").imports([configured(ConfigSource::new().set("port", 1234))])
        }
    }
    let booted = bootstrap::<Root>().await.unwrap();
    let settings: Arc<Settings> = booted.container.resolve().unwrap();
    assert_eq!(settings.port, 1234);
}

#[tokio::test]
async fn invalid_configuration_stops_startup_with_a_clear_error() {
    struct Root;
    impl Module for Root {
        fn definition() -> ModuleDef {
            ModuleDef::new("Root").imports([configured(ConfigSource::new().set("port", 0))])
        }
    }
    let error = bootstrap::<Root>().await.unwrap_err();
    assert!(matches!(&error, DiError::Factory { .. }), "{error}");
    assert!(error.to_string().contains("`port` must be at least 1"), "{error}");
}

// A tiny temp-file helper (no extra dependency).
struct TempToml(std::path::PathBuf);
impl TempToml {
    fn path(&self) -> std::path::PathBuf {
        self.0.clone()
    }
    fn close(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn tempfile_toml(contents: &str) -> TempToml {
    let path = std::env::temp_dir().join(format!(
        "nestrs-config-{}-{:?}.toml",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::File::create(&path)
        .unwrap()
        .write_all(contents.as_bytes())
        .unwrap();
    TempToml(path)
}
