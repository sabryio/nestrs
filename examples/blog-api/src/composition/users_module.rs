use nestrs::prelude::*;
use nestrs_kit::KitModule;

use super::{AuthModule, PersistenceModule, SharedStateModule};
use crate::application::{GetUser, ListUsers, RegisterUser};
use crate::presentation::http::UsersController;

/// The users use cases and their controller. `RegisterUser` is exported for
/// modules that drive it directly (the demo-data seeder).
#[module(
    imports     = [PersistenceModule, AuthModule, KitModule, SharedStateModule],
    providers   = [RegisterUser, GetUser, ListUsers],
    controllers = [UsersController],
    exports     = [RegisterUser],
)]
pub struct UsersModule;
