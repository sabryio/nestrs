use purlin::http::BearerAuth;
use purlin::prelude::*;

use crate::infrastructure::auth::StaticTokenVerifier;

/// Provides the framework's [`BearerAuth`] guard, backed by [`StaticTokenVerifier`].
/// The verifier stays private to the module: importers can use the guard but cannot
/// reach around it to the verifier. Configuration arrives through the global
/// `ConfigModule`.
#[module(
    providers = [StaticTokenVerifier, BearerAuth],
    exports   = [BearerAuth],
)]
pub struct AuthModule;
