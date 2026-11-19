//! Request scopes: per-request and transient values on top of a started app.
//!
//! The app builds every singleton once. [`Scopes`] keeps what is needed to build the
//! short-lived values afterwards (the plan and the built singletons) and is cheap to
//! clone and share across tasks. [`Scopes::open`] makes one [`RequestScope`], e.g. per
//! HTTP request: it resolves singletons from the app, builds request-scoped values at
//! most once, and builds transient values anew on every resolution.
//!
//! Hooks registered by non-singleton constructors are never run (there is no
//! request-level lifecycle), and non-singletons are resolved by root-visible key, so
//! export them from their module.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use crate::descriptor::Lifetime;
use crate::error::{Error, Result};
use crate::extract::{Args, Context, Resolved};
use crate::graph::{Binding, NodeId, NodeKind, Plan};
use crate::key::Key;
use crate::lifecycle::{Lifecycle, Registry};
use crate::shutdown::Shutdowner;
use crate::value::{BoxFuture, Stored, load, store};

struct Shared {
    plan: Arc<Plan>,
    singletons: Vec<Option<Stored>>,
    shutdowner: Shutdowner,
}

/// Everything needed to open request scopes; clone it freely.
#[derive(Clone)]
pub struct Scopes {
    shared: Arc<Shared>,
}

impl Scopes {
    pub(crate) fn new(plan: Arc<Plan>, singletons: Vec<Option<Stored>>, shutdowner: Shutdowner) -> Self {
        Self {
            shared: Arc::new(Shared {
                plan,
                singletons,
                shutdowner,
            }),
        }
    }

    /// Opens an empty scope.
    pub fn open(&self) -> RequestScope {
        RequestScope {
            shared: self.shared.clone(),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// How many request-scoped and transient providers exist.
    pub fn len(&self) -> usize {
        let plan = &self.shared.plan;
        (0..plan.nodes.len())
            .filter(|&n| plan.lifetime(n) != Lifetime::Singleton)
            .count()
    }

    /// Whether the app has no non-singleton providers.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl std::fmt::Debug for Scopes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Scopes({} scoped providers)", self.len())
    }
}

/// The container for one request.
pub struct RequestScope {
    shared: Arc<Shared>,
    cache: Mutex<HashMap<NodeId, Stored>>,
}

impl RequestScope {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<NodeId, Stored>> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn node(&self, key: Key, requester: &str) -> Result<NodeId> {
        self.shared
            .plan
            .root_view
            .get(&key)
            .copied()
            .ok_or_else(|| Error::Missing {
                key,
                requester: requester.into(),
                path: Box::default(),
            })
    }

    /// Supplies the value of a [`Options::seeded`](crate::Options::seeded) provider.
    ///
    /// # Errors
    ///
    /// If `T` is not declared, or is not request-scoped.
    pub fn seed<T: ?Sized + Send + Sync + 'static>(&self, value: Arc<T>) -> Result<()> {
        let key = Key::of::<T>();
        let node = self.node(key, "RequestScope::seed")?;
        if self.shared.plan.lifetime(node) != Lifetime::Request {
            return Err(Error::internal(format!(
                "`{key}` is not request-scoped, so it cannot be seeded"
            )));
        }
        self.lock().insert(node, store(value));
        Ok(())
    }

    /// Resolves `T`: a singleton, this scope's instance, or a fresh transient one.
    ///
    /// # Errors
    ///
    /// If `T` is unknown, or a constructor fails.
    pub async fn resolve<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Arc<T>> {
        self.resolve_key(Key::of::<T>()).await
    }

    /// Like [`resolve`](Self::resolve), for a value registered under `name`.
    ///
    /// # Errors
    ///
    /// As [`resolve`](Self::resolve).
    pub async fn resolve_named<T: ?Sized + Send + Sync + 'static>(&self, name: &'static str) -> Result<Arc<T>> {
        self.resolve_key(Key::named::<T>(name)).await
    }

    async fn resolve_key<T: ?Sized + Send + Sync + 'static>(&self, key: Key) -> Result<Arc<T>> {
        let node = self.node(key, "RequestScope::resolve")?;
        let stored = self.resolve_node(node).await?;
        load::<T>(&stored).ok_or_else(|| Error::internal(format!("`{key}` has an unexpected type")))
    }

    fn resolve_node(&self, node: NodeId) -> BoxFuture<'_, Result<Stored>> {
        Box::pin(async move {
            let plan = &self.shared.plan;
            let lifetime = plan.lifetime(node);
            if lifetime == Lifetime::Singleton {
                return self.shared.singletons[node]
                    .clone()
                    .ok_or_else(|| Error::internal("a singleton was not built before the scope opened"));
            }
            if lifetime == Lifetime::Request
                && let Some(hit) = self.lock().get(&node)
            {
                return Ok(hit.clone());
            }

            let mut resolved = Vec::new();
            for binding in &plan.nodes[node].args {
                resolved.push(match binding {
                    Binding::One(dep) => Resolved::One(self.resolve_node(*dep).await?),
                    Binding::Maybe(None) => Resolved::Maybe(None),
                    Binding::Maybe(Some(dep)) => Resolved::Maybe(Some(self.resolve_node(*dep).await?)),
                    Binding::Many(deps) => {
                        let mut all = Vec::with_capacity(deps.len());
                        for dep in deps {
                            all.push(self.resolve_node(*dep).await?);
                        }
                        Resolved::Many(all)
                    }
                });
            }

            let NodeKind::Provider(index) = plan.nodes[node].kind else {
                return Err(Error::internal("only providers can be resolved from a scope"));
            };
            let provider = &plan.providers[index].provider;
            let key = provider.key;
            let origin = provider.origin;
            let context = Context {
                lifecycle: Lifecycle::new(Arc::new(Registry::default()), format!("{key} by {origin}")),
                shutdowner: self.shared.shutdowner.clone(),
            };
            let stored = (provider.build.clone())(&mut Args::new(resolved), &context)?
                .await
                .map_err(|source| Error::Build {
                    key: key.to_string().into(),
                    origin: origin.to_string().into(),
                    source,
                })?;

            Ok(match lifetime {
                // Keep the first instance if a concurrent resolution got there first.
                Lifetime::Request => self.lock().entry(node).or_insert(stored).clone(),
                _ => stored,
            })
        })
    }
}

impl std::fmt::Debug for RequestScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestScope")
            .field("cached", &self.lock().len())
            .finish_non_exhaustive()
    }
}
