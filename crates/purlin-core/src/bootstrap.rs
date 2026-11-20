use std::sync::Arc;

use crate::graph::Plan;
use crate::transport::MountedController;
use crate::{Container, DiError, Module, ModuleRef, Overrides, ScopedProviders, Shutdown, Token};

/// The result of a successful bootstrap.
#[derive(Debug)]
pub struct Bootstrapped {
    /// Every singleton provider, built.
    pub container: Arc<Container>,
    /// Recipes for request-scoped and transient providers; see [`RequestScope`](crate::RequestScope).
    pub scoped: Arc<ScopedProviders>,
    /// Every controller, mounted for whichever transport it targets.
    pub controllers: Vec<MountedController>,
    /// Module names in dependency order (imports first).
    pub modules: Vec<&'static str>,
    /// Destroy hooks, to be run once at shutdown; see [`Shutdown::run`].
    pub shutdown: Shutdown,
}

/// Validates the module graph rooted at `M` and builds it.
///
/// Nothing is constructed until the whole graph has been checked, so a wiring
/// mistake (missing provider, unexported dependency, cycle) is reported without any
/// side effects from half-built services.
pub async fn bootstrap<M: Module>() -> Result<Bootstrapped, DiError> {
    bootstrap_with::<M>(Overrides::new()).await
}

/// Like [`bootstrap`], with replacement providers applied first; see [`Overrides`].
pub async fn bootstrap_with<M: Module>(overrides: Overrides) -> Result<Bootstrapped, DiError> {
    let plan = Plan::resolve(ModuleRef::of::<M>(), overrides)?;

    let (container, shutdown, scopes) = crate::engine::start(&plan).await?;

    let modules = plan.modules.iter().map(|module| module.name).collect();
    let scoped = ScopedProviders::new(scopes);
    let container = Arc::new(container);
    let mut controllers = Vec::new();
    for node in plan.providers {
        if let Some(controller) = node.controller {
            let token = node.provider.token();
            let mounted = (controller.mount)(&container).map_err(|error| attribute(token, error))?;
            controllers.push(MountedController::new(token, controller.transport, mounted));
        }
    }

    Ok(Bootstrapped {
        container,
        scoped: Arc::new(scoped),
        controllers,
        modules,
        shutdown,
    })
}

/// Pins an error to the provider that raised it, unless it already is.
fn attribute(provider: Token, error: DiError) -> DiError {
    match error {
        already @ DiError::Factory { .. } => already,
        other => DiError::Factory {
            provider,
            message: other.to_string(),
        },
    }
}
