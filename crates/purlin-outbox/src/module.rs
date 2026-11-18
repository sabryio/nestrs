//! Packages the relay as a module.

use purlin::prelude::*;

use crate::relay::{OutboxConfig, OutboxRelay};

/// Import it to run the relay.
///
/// The relay needs two ports, which you provide from modules it imports:
///
/// * [`OutboxStorePort`](crate::OutboxStorePort): where messages wait. With SeaORM, list
///   [`SeaOutboxStore`](crate::SeaOutboxStore) in your persistence module and export the port.
/// * [`OutboxPublisherPort`](crate::OutboxPublisherPort): where they go. With a
///   `purlin-jobs` queue, list [`QueuePublisher`](crate::QueuePublisher) in a module that
///   imports `JobsModule`.
///
/// ```ignore
/// #[module(imports = [
///     OutboxModule::for_root(OutboxConfig::default(), [ModuleRef::of::<PersistenceModule>(), ModuleRef::of::<MessagingModule>()]),
/// ])]
/// struct AppModule;
/// ```
pub struct OutboxModule;

impl OutboxModule {
    /// The relay with `config`, resolving its ports from `imports`.
    pub fn for_root(config: OutboxConfig, imports: impl IntoIterator<Item = ModuleRef>) -> ModuleRef {
        Self::with_config(Provider::value(config), imports)
    }

    /// Like [`for_root`](Self::for_root), with the settings coming from a provider of
    /// [`OutboxConfig`], for when they depend on your own configuration.
    ///
    /// # Panics
    ///
    /// If `config` does not provide an [`OutboxConfig`]: a wiring mistake, reported at module
    /// construction.
    pub fn with_config(config: Provider, imports: impl IntoIterator<Item = ModuleRef>) -> ModuleRef {
        assert_eq!(
            config.token(),
            Token::of::<OutboxConfig>(),
            "OutboxModule needs a provider of `OutboxConfig`"
        );
        ModuleDef::new("OutboxModule")
            .imports(imports)
            .providers([
                config,
                Provider::of::<OutboxRelay>()
                    .with_init::<OutboxRelay>()
                    .with_destroy::<OutboxRelay>(),
            ])
            .exports([Token::of::<OutboxRelay>()])
            .into_ref()
    }
}
