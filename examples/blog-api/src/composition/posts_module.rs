use purlin::prelude::*;
use purlin_kit::KitModule;

use super::{AuthModule, PersistenceModule, SharedStateModule};
use crate::application::{DeletePost, GetPost, ListPosts, PublishPost, WatchPosts};
use crate::infrastructure::events::{BroadcastPostEvents, post_events};
use crate::presentation::http::PostsController;

/// The posts use cases and their controller. Publishing needs the user repository
/// *port* (to check authors), which `PersistenceModule` exports.
#[module(
    imports     = [PersistenceModule, AuthModule, KitModule, SharedStateModule],
    providers   = [BroadcastPostEvents, post_events, PublishPost, GetPost, ListPosts, DeletePost, WatchPosts],
    controllers = [PostsController],
    exports     = [PublishPost],
)]
pub struct PostsModule;
