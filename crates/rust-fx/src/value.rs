//! Type-erased storage for provided values.

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// A heap-allocated, sendable future; the shape constructors and hooks return.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A value held by the runtime: an `Arc<T>` (with `T` possibly `dyn Trait`) boxed
/// behind `Any`, because `Arc<dyn Trait>` cannot be downcast directly.
pub(crate) type Stored = Arc<dyn Any + Send + Sync>;

pub(crate) fn store<T: ?Sized + Send + Sync + 'static>(value: Arc<T>) -> Stored {
    Arc::new(value)
}

pub(crate) fn load<T: ?Sized + Send + Sync + 'static>(stored: &Stored) -> Option<Arc<T>> {
    stored.downcast_ref::<Arc<T>>().cloned()
}
