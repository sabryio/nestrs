//! Layered, typed, validated configuration.
//!
//! Settings come from, lowest priority first:
//!
//! 1. the defaults in your type (`#[serde(default)]`) and any [`ConfigSource::with_default`] values,
//! 2. config files ([`ConfigSource::file`], TOML),
//! 3. environment variables ([`ConfigSource::env_prefix`]; `BLOG_API_TOKEN` sets `api_token`,
//!    `BLOG_DB__URL` sets `db.url`),
//! 4. explicit [`ConfigSource::set`] overrides (handy in tests).
//!
//! The merged result is deserialised into your struct and validated with
//! [`validator`]. Every problem is reported at once, at startup, as one
//! [`ConfigError`]; nothing half-configured ever runs.
//!
//! ```ignore
//! #[derive(Debug, Clone, Deserialize, Validate)]
//! #[serde(default)]
//! struct AppConfig {
//!     #[validate(length(min = 8))]
//!     api_token: String,
//!     port: u16,
//! }
//!
//! #[module(imports = [ConfigModule::for_root::<AppConfig>(
//!     ConfigSource::new().file("app.toml").env_prefix("APP"),
//! )])]
//! struct AppModule;
//! ```

#![forbid(unsafe_code)]

use std::path::PathBuf;

use purlin_core::{ModuleDef, ModuleRef, Provider, Token};
use serde::de::DeserializeOwned;
use validator::Validate;

pub use config::Value;

/// Why configuration could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A source could not be read or merged, or the result does not fit the type.
    #[error("invalid configuration: {0}")]
    Load(String),
    /// The values loaded but break the type's validation rules.
    #[error("configuration failed validation: {}", .0.join("; "))]
    Invalid(Vec<String>),
}

/// Where configuration comes from. Cheap to build, loaded once at startup.
#[derive(Debug, Clone, Default)]
pub struct ConfigSource {
    defaults: Vec<(String, Value)>,
    files: Vec<(PathBuf, bool)>,
    env_prefix: Option<String>,
    env_vars: Option<std::collections::HashMap<String, String>>,
    overrides: Vec<(String, Value)>,
}

impl ConfigSource {
    /// An empty source: only the type's own `serde` defaults apply.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets a default for `key` (dotted for nesting: `"db.pool_size"`).
    pub fn with_default(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.defaults.push((key.to_owned(), value.into()));
        self
    }

    /// Reads a TOML file. A missing file is skipped: configuration files are
    /// optional, environment variables can supply everything.
    pub fn file(mut self, path: impl Into<PathBuf>) -> Self {
        self.files.push((path.into(), false));
        self
    }

    /// Like [`file`](Self::file), but a missing file is an error.
    pub fn required_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.files.push((path.into(), true));
        self
    }

    /// Reads environment variables starting with `PREFIX_`; `__` nests
    /// (`APP_DB__URL` -> `db.url`). Values are parsed to the field's type.
    pub fn env_prefix(mut self, prefix: &str) -> Self {
        self.env_prefix = Some(prefix.to_owned());
        self
    }

    /// Reads the prefixed variables from `vars` instead of the process environment.
    ///
    /// Useful in tests (editing the real environment is `unsafe` in edition 2024) and
    /// when embedding the application. Combine with [`env_prefix`](Self::env_prefix).
    pub fn env_vars<K, V>(mut self, vars: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        self.env_vars = Some(vars.into_iter().map(|(k, v)| (k.into(), v.into())).collect());
        self
    }

    /// Forces `key` to `value`, above every other source.
    pub fn set(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.overrides.push((key.to_owned(), value.into()));
        self
    }

    /// Merges the sources, deserialises into `T` and validates it.
    pub fn load<T>(&self) -> Result<T, ConfigError>
    where
        T: DeserializeOwned + Validate,
    {
        let load = |error: config::ConfigError| ConfigError::Load(error.to_string());

        let mut builder = config::Config::builder();
        for (key, value) in &self.defaults {
            builder = builder.set_default(key, value.clone()).map_err(load)?;
        }
        for (path, required) in &self.files {
            builder = builder.add_source(
                config::File::from(path.as_path())
                    .format(config::FileFormat::Toml)
                    .required(*required),
            );
        }
        if let Some(prefix) = &self.env_prefix {
            builder = builder.add_source(
                config::Environment::with_prefix(prefix)
                    .prefix_separator("_")
                    .separator("__")
                    .try_parsing(true)
                    .source(self.env_vars.clone()),
            );
        }
        for (key, value) in &self.overrides {
            builder = builder.set_override(key, value.clone()).map_err(load)?;
        }

        let value: T = builder.build().map_err(load)?.try_deserialize().map_err(load)?;
        value
            .validate()
            .map_err(|errors| ConfigError::Invalid(describe(&errors)))?;
        Ok(value)
    }
}

/// `field: message` for every failed rule, nested fields dotted.
fn describe(errors: &validator::ValidationErrors) -> Vec<String> {
    let mut lines: Vec<String> = errors
        .field_errors()
        .into_iter()
        .flat_map(|(field, failures)| {
            failures.iter().map(move |failure| {
                let reason = failure.message.as_deref().unwrap_or(&failure.code);
                format!("`{field}` {reason}")
            })
        })
        .collect();
    lines.sort();
    lines
}

/// Provides a validated, typed configuration `T` to the whole application.
///
/// The module is global: any provider can depend on `Arc<T>` without importing it.
/// Loading happens when the provider is built, so a bad configuration stops
/// startup with an error naming the provider and every problem found.
pub struct ConfigModule;

impl ConfigModule {
    /// Builds the module that loads `T` from `source`.
    pub fn for_root<T>(source: ConfigSource) -> ModuleRef
    where
        T: DeserializeOwned + Validate + Send + Sync + 'static,
    {
        ModuleDef::new("ConfigModule")
            .global()
            .providers([Provider::factory::<T>(Vec::new(), move |_| {
                let loaded = source.load::<T>().map_err(purlin_core::DiError::custom);
                Box::pin(async move { loaded })
            })])
            .exports([Token::of::<T>()])
            .into_ref()
    }
}
