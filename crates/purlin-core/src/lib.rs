//! # purlin-core
//!
//! The transport-agnostic heart of purlin: a dependency-injection container, a
//! module graph, and the bootstrap that validates and builds it.
//!
//! This crate knows nothing about HTTP, `axum` or proc macros. Everything above it
//! (attribute macros, the HTTP adapter) is built on the small public contracts
//! defined here: [`Injectable`], [`Module`], [`Controller`] and [`Transport`].
//!
//! ## Model
//!
//! - A [`Provider`] is a recipe for a singleton, keyed by a [`Token`] (a concrete
//!   type or a `dyn Trait`).
//! - A [`Module`] bundles providers and controllers and decides what it exports.
//! - [`bootstrap`] checks the whole graph (missing, invisible, duplicate and cyclic
//!   dependencies), builds providers in dependency order, and mounts controllers.
//!
//! Wiring mistakes surface as a [`DiError`] at startup, never as a runtime panic in
//! the middle of a request.

#![forbid(unsafe_code)]

mod bootstrap;
mod container;
mod dependency;
mod engine;
mod error;
mod graph;
mod lifecycle;
mod module;
mod overrides;
mod provider;
mod scope;
mod token;
mod transport;

pub use bootstrap::{Bootstrapped, bootstrap, bootstrap_with};
pub use container::Container;
#[doc(hidden)]
pub use dependency::__private;
pub use dependency::{Dependency, ProviderSource};
pub use error::DiError;
pub use lifecycle::{OnModuleDestroy, OnModuleInit, Shutdown};
pub use module::{Module, ModuleDef, ModuleRef};
pub use overrides::Overrides;
pub use provider::{BoxFuture, Injectable, Multi, Provider};
pub use scope::{Lifetime, RequestScope, ScopedProviders};
pub use token::Token;
pub use transport::{Controller, ControllerEntry, MountedController, Transport};
