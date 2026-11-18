//! # purlin-outbox
//!
//! The **transactional outbox**: record the event in the *same database transaction* as the
//! data it describes, and let a relay deliver it afterwards. That removes the dual-write bug,
//! where a service commits an order and then crashes before announcing it (or announces it and
//! then rolls back).
//!
//! ```text
//!  request ── BEGIN ── INSERT order ── INSERT outbox row ── COMMIT      (atomic)
//!                                                              │
//!  relay  ── claim ── publish ── delete row  ◄─────────────────┘      (at least once)
//! ```
//!
//! | Piece | Job |
//! |---|---|
//! | [`NewMessage`], [`OutboxMessage`] | what is recorded / what the relay sees |
//! | [`OutboxStore`] | port: claim, complete, release, bury (the relay's view) |
//! | [`OutboxPublisher`] | port: deliver one message (`QueuePublisher` for `purlin-jobs`) |
//! | [`OutboxRelay`] | claims, publishes, retries with back-off, buries; stops on shutdown |
//! | [`MemoryOutboxStore`] | for tests (not transactional) |
//! | `Outbox`, `SeaOutboxStore`, `CreateOutbox` | SeaORM: write inside a transaction, store, migration |
//! | [`OutboxModule`] | wires the relay |
//!
//! ## Guarantees
//!
//! * A message exists **iff** the transaction that recorded it committed.
//! * Delivery is **at least once**. Consumers deduplicate on [`OutboxMessage::id`], which is the
//!   same on every redelivery ([`OutboxEnvelope`] carries it to queue handlers).
//! * Messages with the same key are delivered **in recording order, one at a time**. Messages
//!   without a key may overtake each other, which is what lets them be delivered in parallel
//!   and never stuck behind a failing neighbour.
//! * A message that keeps failing is retried with exponential back-off and then buried; it
//!   stays in the table for inspection and can be [requeued](OutboxStore::requeue_dead).
//!
//! ## Not guaranteed
//!
//! * Ordering **across** keys.
//! * Ordering of messages with one key recorded by *concurrent* transactions: the order is
//!   the order of the database's sequence, not of commit. Record them from the code that
//!   already serialises writes to that aggregate (a row lock, a version check) and it holds.

#![forbid(unsafe_code)]

mod error;
mod message;
mod module;
mod publisher;
mod relay;
mod store;

#[cfg(feature = "sea-orm")]
mod sea;

pub use error::{OutboxError, StoreError};
pub use message::{NewMessage, OutboxEnvelope, OutboxMessage};
pub use module::OutboxModule;
#[cfg(feature = "jobs")]
pub use publisher::QueuePublisher;
pub use publisher::{OutboxPublisher, OutboxPublisherPort, PublishError};
pub use relay::{OutboxConfig, OutboxRelay, RelayStats};
#[cfg(feature = "sea-orm")]
pub use sea::{CreateOutbox, Outbox, SeaOutboxStore};
pub use store::{MemoryOutboxStore, OutboxStore, OutboxStorePort};
