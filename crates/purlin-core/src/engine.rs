//! Execution of a resolved plan on `fx`.
//!
//! purlin keeps what is purlin's: the module graph, visibility and export rules, multi-bindings,
//! overrides and their typed [`DiError`]s (all in `graph.rs`). What it hands to `fx` is the
//! *result*: a flat, validated list of singleton providers in dependency order. `fx` then does
//! what it is for: builds them, runs start hooks once everything exists, and stops in reverse.
//!
//! The seam is [`fx::Provider::dynamic`]: a purlin provider knows its [`Token`]s and factory at
//! runtime, not as Rust types.

use std::sync::Arc;

use fx::{App, BoxError, DepKind, DepSpec, Key, Options, Resolved};

use crate::graph::Plan;
use crate::lifecycle::Shutdown;
use crate::{Container, DiError, Lifetime, Token};

/// The fx key of a token. Tokens and keys are both a `TypeId` plus a name for messages.
pub(crate) fn key_of(token: Token) -> Key {
    Key::from_parts(token.id(), token.name(), None, false)
}

/// A hook failure, carried through fx so that the failing provider can be reported.
#[derive(Debug)]
pub(crate) struct HookFailed {
    pub(crate) token: Token,
    pub(crate) message: String,
}

impl std::fmt::Display for HookFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for HookFailed {}

fn attribute(provider: Token, error: DiError) -> DiError {
    match error {
        already @ DiError::Factory { .. } => already,
        other => DiError::Factory {
            provider,
            message: other.to_string(),
        },
    }
}

/// Translates every provider of the plan into an fx provider, keeping its lifetime.
fn options(plan: &Plan) -> Options {
    let mut options = Options::new();
    for &index in &plan.order {
        let provider = plan.providers[index].provider.clone();
        let lifetime = provider.lifetime();
        let token = provider.token();
        // A per-request dependency is resolved when a request scope opens, not at startup.
        let dependencies: Vec<Token> = provider
            .dependencies()
            .iter()
            .copied()
            .filter(|dependency| !provider.is_per_request(dependency))
            .collect();
        let specs = dependencies
            .iter()
            .map(|&dependency| DepSpec {
                key: key_of(dependency),
                kind: DepKind::Required,
            })
            .collect();

        options = options.constructor(fx::Provider::dynamic(
            key_of(token),
            specs,
            "purlin-core",
            lifetime,
            move |arguments, context| {
                let provider = provider.clone();
                let dependencies = dependencies.clone();
                let lifecycle = context.lifecycle.clone();
                Box::pin(async move {
                    let mut container = Container::new();
                    for (dependency, argument) in dependencies.iter().zip(arguments) {
                        if let Resolved::One(stored) = argument {
                            container.insert_raw(*dependency, stored);
                        }
                    }
                    let built = provider
                        .build(&container)
                        .await
                        .map_err(|error| Box::new(attribute(token, error)) as BoxError)?;

                    // Hooks resolve the provider's own value, as they always have.
                    let mut own = Container::new();
                    own.insert_raw(token, built.clone());
                    let own = Arc::new(own);
                    for hook in provider.init_hooks() {
                        let (hook, own) = (Arc::clone(hook), Arc::clone(&own));
                        lifecycle.on_start(move || async move {
                            hook(&own).await.map_err(|error| HookFailed {
                                token,
                                message: error.to_string(),
                            })
                        });
                    }
                    for hook in provider.destroy_hooks() {
                        let (hook, own) = (Arc::clone(hook), Arc::clone(&own));
                        lifecycle.on_stop(move || async move {
                            hook(&own).await.map_err(|error| HookFailed {
                                token,
                                message: error.to_string(),
                            })
                        });
                    }
                    Ok(built)
                })
            },
        ));
    }
    options
}

/// Builds every singleton, runs init hooks, and returns the container plus the shutdown handle.
///
/// # Errors
///
/// The same [`DiError`]s as the built-in engine: `Factory` for a failed build, `Hook` for a
/// failed init (after the hooks that had run are torn down).
pub(crate) async fn start(plan: &Plan) -> Result<(Container, Shutdown, fx::Scopes), DiError> {
    let mut app = App::new().options(options(plan)).eager();
    app.start().await.map_err(into_di)?;

    let mut container = Container::new();
    for &index in &plan.order {
        let provider = &plan.providers[index].provider;
        if provider.lifetime() != Lifetime::Singleton {
            continue;
        }
        let token = provider.token();
        let stored = app.get_erased(key_of(token)).await.map_err(into_di)?;
        container.insert_raw(token, stored);
    }
    let scopes = app.scopes().map_err(into_di)?;
    Ok((container, Shutdown::on_fx(app), scopes))
}

/// Maps an fx error back to the error type purlin has always returned.
pub(crate) fn into_di(error: fx::Error) -> DiError {
    match error {
        fx::Error::Build { source, .. } | fx::Error::Hook { source, .. } => match source.downcast::<DiError>() {
            Ok(di) => *di,
            Err(source) => match source.downcast::<HookFailed>() {
                Ok(hook) => DiError::Hook {
                    provider: hook.token,
                    phase: "init",
                    message: hook.message,
                },
                Err(other) => DiError::custom(other),
            },
        },
        fx::Error::Cycle { path } => DiError::CircularDependency { path },
        other => DiError::custom(other),
    }
}

/// Maps the errors of an fx stop to purlin's `destroy` hook errors.
pub(crate) fn stop_errors(error: fx::Error) -> Vec<DiError> {
    let errors = match error {
        fx::Error::Stop { errors } => errors,
        other => vec![other],
    };
    errors
        .into_iter()
        .map(|error| match error {
            fx::Error::Hook { source, .. } => match source.downcast::<HookFailed>() {
                Ok(hook) => DiError::Hook {
                    provider: hook.token,
                    phase: "destroy",
                    message: hook.message,
                },
                Err(other) => DiError::custom(other),
            },
            other => DiError::custom(other),
        })
        .collect()
}
