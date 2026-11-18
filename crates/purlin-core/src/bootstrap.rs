use std::sync::Arc;

use crate::graph::Plan;
use crate::lifecycle::Hook;
use crate::transport::MountedController;
use crate::{Container, DiError, Lifetime, Module, ModuleRef, Overrides, ScopedProviders, Shutdown, Token};

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

    let mut container = Container::new();
    for &index in &plan.order {
        let provider = &plan.providers[index].provider;
        if provider.lifetime() != Lifetime::Singleton {
            continue;
        }
        let built = provider
            .build(&container)
            .await
            .map_err(|error| attribute(provider.token(), error))?;
        container.insert_raw(provider.token(), built);
    }

    // Init hooks run once everything exists, dependencies first. A provider's
    // destroy hooks are registered only after its init succeeded, so a failed start
    // tears down exactly what had started.
    let mut destroy: Vec<(Token, Hook)> = Vec::new();
    for &index in &plan.order {
        let provider = &plan.providers[index].provider;
        if provider.lifetime() != Lifetime::Singleton {
            continue;
        }
        for hook in provider.init_hooks() {
            if let Err(error) = hook(&container).await {
                destroy.reverse();
                // Teardown is best effort here: the init failure is the error to report.
                let _ = Shutdown::new(destroy).run(&container).await;
                return Err(DiError::Hook {
                    provider: provider.token(),
                    phase: "init",
                    message: error.to_string(),
                });
            }
        }
        destroy.extend(
            provider
                .destroy_hooks()
                .iter()
                .map(|hook| (provider.token(), Arc::clone(hook))),
        );
    }
    destroy.reverse();

    let modules = plan.modules.iter().map(|module| module.name).collect();
    let scoped = ScopedProviders::new(
        plan.providers
            .iter()
            .filter(|node| node.provider.lifetime() != Lifetime::Singleton)
            .map(|node| (node.provider.token(), node.provider.clone()))
            .collect(),
    );
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
        shutdown: Shutdown::new(destroy),
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
