use std::any::{Any, TypeId};
use std::sync::Arc;

use crate::{Container, DiError, Injectable, Provider, Token};

/// A way of exposing controllers to the outside world (HTTP, gRPC, a CLI, ..).
///
/// The core never looks inside a transport. A transport declares what a mounted
/// controller *is* (`Mounted`, e.g. an `axum::Router`) and the matching adapter
/// crate collects those values after bootstrap. Adding a transport therefore never
/// touches this crate.
pub trait Transport: 'static {
    /// What mounting one controller produces.
    type Mounted: Send + 'static;
}

pub(crate) type MountFn = Arc<dyn Fn(&Container) -> Result<Box<dyn Any + Send>, DiError> + Send + Sync>;

/// A controller registration: the provider that builds it plus how to mount it.
#[derive(Clone)]
pub struct ControllerEntry {
    pub(crate) provider: Provider,
    pub(crate) transport: TypeId,
    pub(crate) mount: MountFn,
}

impl ControllerEntry {
    /// Registers a controller for transport `T`.
    ///
    /// `provider` builds the controller itself; `mount` runs after the whole graph
    /// is built and turns the resolved controller into the transport's artefact.
    pub fn new<T: Transport>(
        provider: Provider,
        mount: impl Fn(&Container) -> Result<T::Mounted, DiError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            provider,
            transport: TypeId::of::<T>(),
            mount: Arc::new(move |container: &Container| {
                mount(container).map(|mounted| Box::new(mounted) as Box<dyn Any + Send>)
            }),
        }
    }
}

/// A transport-bound entry point that a module can list under `controllers`.
///
/// `#[controller]` implements this; the generated `entry` picks the right
/// transport so `#[module]` stays transport-agnostic.
pub trait Controller: Injectable {
    /// The registration for this controller.
    fn entry() -> ControllerEntry;
}

/// A controller after mounting, still tagged with the transport that made it.
#[derive(Debug)]
pub struct MountedController {
    /// The controller type.
    pub controller: Token,
    transport: TypeId,
    mounted: Box<dyn Any + Send>,
}

impl MountedController {
    pub(crate) fn new(controller: Token, transport: TypeId, mounted: Box<dyn Any + Send>) -> Self {
        Self {
            controller,
            transport,
            mounted,
        }
    }

    /// Extracts the mounted artefact if this controller belongs to transport `T`;
    /// otherwise hands the controller back untouched.
    pub fn for_transport<T: Transport>(self) -> Result<T::Mounted, Self> {
        if self.transport != TypeId::of::<T>() {
            return Err(self);
        }
        match self.mounted.downcast::<T::Mounted>() {
            Ok(mounted) => Ok(*mounted),
            Err(mounted) => Err(Self {
                controller: self.controller,
                transport: self.transport,
                mounted,
            }),
        }
    }
}
