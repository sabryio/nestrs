//! Transactions: one per request ([`Transactional`]) or explicit ([`UnitOfWork`]).
//!
//! The request-scoped one is kept in a Tokio task-local, so it follows the request's task
//! and nothing has to be passed through call chains. The consequence to know: work moved
//! onto `tokio::spawn` does not see it and uses the pool instead.

use std::sync::Arc;

#[cfg(feature = "http")]
use purlin::http::axum::extract::Request;
#[cfg(feature = "http")]
use purlin::http::axum::middleware::Next;
#[cfg(feature = "http")]
use purlin::http::axum::response::{IntoResponse, Response};
#[cfg(feature = "http")]
use purlin::prelude::*;
#[cfg(feature = "http")]
use sea_orm::TransactionTrait;
use sea_orm::{DatabaseConnection, DatabaseTransaction};
#[cfg(feature = "http")]
use tokio::sync::Mutex;

#[cfg(feature = "http")]
use crate::Database;
use crate::{DbError, Executor};

#[cfg(feature = "http")]
tokio::task_local! {
    static ACTIVE: Arc<Slot>;
}

/// The (lazily opened) transaction of one request.
#[cfg(feature = "http")]
pub(crate) struct Slot {
    database: u64,
    state: Mutex<State>,
}

#[cfg(feature = "http")]
enum State {
    /// Nothing has needed the database yet, so no `BEGIN` was sent.
    Idle,
    Open(Arc<DatabaseTransaction>),
    Finished,
}

#[cfg(feature = "http")]
impl Slot {
    #[cfg(feature = "http")]
    fn new(database: u64) -> Self {
        Self {
            database,
            state: Mutex::new(State::Idle),
        }
    }

    /// The slot of the request being served, if it belongs to `database`.
    pub(crate) fn current(database: u64) -> Option<Arc<Slot>> {
        ACTIVE
            .try_with(Arc::clone)
            .ok()
            .filter(|slot| slot.database == database)
    }

    /// The request's transaction, opened on first use.
    pub(crate) async fn executor(&self, connection: &DatabaseConnection) -> Result<Executor, DbError> {
        let mut state = self.state.lock().await;
        match &*state {
            State::Open(transaction) => Ok(Executor::transaction(Arc::clone(transaction))),
            State::Finished => Err(DbError::Transaction(
                "the request's transaction has already finished".to_owned(),
            )),
            State::Idle => {
                let transaction = Arc::new(
                    connection
                        .begin()
                        .await
                        .map_err(|error| DbError::Transaction(error.to_string()))?,
                );
                *state = State::Open(Arc::clone(&transaction));
                Ok(Executor::transaction(transaction))
            }
        }
    }

    #[cfg(feature = "http")]
    async fn finish(&self, commit: bool) -> Result<(), DbError> {
        let mut state = self.state.lock().await;
        match std::mem::replace(&mut *state, State::Finished) {
            State::Idle | State::Finished => Ok(()),
            State::Open(transaction) => {
                // Handlers are done, so this is the only handle unless something leaked one
                // (a spawned task). Then the transaction rolls back when the last one drops.
                let transaction = Arc::try_unwrap(transaction).map_err(|_| {
                    DbError::Transaction("the transaction is still in use; it will roll back".to_owned())
                })?;
                let outcome = if commit {
                    transaction.commit().await
                } else {
                    transaction.rollback().await
                };
                outcome.map_err(|error| DbError::Transaction(error.to_string()))
            }
        }
    }
}

/// Without the `http` feature nothing opens a per-request transaction, so there is never a
/// slot to join; explicit transactions go through [`UnitOfWork`].
#[cfg(not(feature = "http"))]
pub(crate) struct Slot;

#[cfg(not(feature = "http"))]
impl Slot {
    pub(crate) fn current(_database: u64) -> Option<Arc<Slot>> {
        None
    }

    pub(crate) async fn executor(&self, _connection: &DatabaseConnection) -> Result<Executor, DbError> {
        Err(DbError::Transaction(
            "request transactions need the `http` feature".to_owned(),
        ))
    }
}

/// Interceptor: runs the request in one transaction.
///
/// The transaction opens the first time a repository asks for an [`Executor`], so routes
/// that never touch the database cost nothing. When the response is a success or a
/// redirect the transaction commits; a `4xx` or `5xx` response rolls it back, so
/// returning an error from a handler undoes its writes without any code for it.
///
/// Attach it with `#[interceptor(Transactional)]` on a write route. It is provided and
/// exported by [`SeaOrmModule`](crate::SeaOrmModule).
///
/// With in-memory SQLite the pool has a single connection, so while a transaction is open
/// other database users (a health probe, say) wait for it. Real databases have no such limit.
#[cfg(feature = "http")]
#[injectable]
pub struct Transactional {
    database: Arc<Database>,
}

#[cfg(feature = "http")]
impl Interceptor for Transactional {
    async fn intercept(&self, request: Request, next: Next) -> Response {
        let slot = Arc::new(Slot::new(self.database.id()));
        let response = ACTIVE.scope(Arc::clone(&slot), next.run(request)).await;
        let status = response.status();
        let commit = !(status.is_client_error() || status.is_server_error());
        match slot.finish(commit).await {
            Ok(()) => response,
            Err(error) if commit => {
                // The client was about to be told "done"; it was not.
                tracing::error!(%error, "transaction could not be committed");
                HttpError::internal(error.to_string()).into_response()
            }
            Err(error) => {
                tracing::warn!(%error, "transaction could not be rolled back cleanly");
                response
            }
        }
    }
}

/// An explicit transaction, for work that is not a request (a job, a CLI command).
///
/// Dropping it without [`commit`](Self::commit) rolls back.
pub struct UnitOfWork {
    transaction: Arc<DatabaseTransaction>,
}

impl UnitOfWork {
    pub(crate) fn new(transaction: DatabaseTransaction) -> Self {
        Self {
            transaction: Arc::new(transaction),
        }
    }

    /// A handle that runs statements inside this transaction.
    pub fn executor(&self) -> Executor {
        Executor::transaction(Arc::clone(&self.transaction))
    }

    /// Makes the changes permanent. Every [`Executor`] taken from this unit must be dropped
    /// first.
    pub async fn commit(self) -> Result<(), DbError> {
        self.finish(true).await
    }

    /// Discards the changes.
    pub async fn rollback(self) -> Result<(), DbError> {
        self.finish(false).await
    }

    async fn finish(self, commit: bool) -> Result<(), DbError> {
        let transaction = Arc::try_unwrap(self.transaction)
            .map_err(|_| DbError::Transaction("an executor of this transaction is still alive".to_owned()))?;
        let outcome = if commit {
            transaction.commit().await
        } else {
            transaction.rollback().await
        };
        outcome.map_err(|error| DbError::Transaction(error.to_string()))
    }
}
