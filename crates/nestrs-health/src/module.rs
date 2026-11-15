//! Packages the probes as a module.

use nestrs::prelude::*;

use crate::checker::{HealthChecker, HealthConfig};
use crate::controller::HealthController;
use crate::indicator::HealthIndicator;

/// Import it to get `/health/live` and `/health/ready`.
///
/// `imports = [HealthModule]` uses the defaults; `HealthModule::for_root(config)`
/// tunes them. It declares the (possibly empty) set of indicators, so an app with
/// none still starts, and ready means "up".
pub struct HealthModule;

impl HealthModule {
    /// The module with custom settings.
    pub fn for_root(config: HealthConfig) -> ModuleRef {
        Self::definition_with(config).into_ref()
    }

    fn definition_with(config: HealthConfig) -> ModuleDef {
        ModuleDef::new("HealthModule")
            .providers([
                Provider::value(config),
                Provider::multi::<dyn HealthIndicator>(),
                Provider::of::<HealthChecker>(),
            ])
            .controllers([<HealthController as Controller>::entry()])
    }
}

impl Module for HealthModule {
    fn definition() -> ModuleDef {
        Self::definition_with(HealthConfig::default())
    }
}
