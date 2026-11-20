//! Type-erased storage for provided values.

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// A heap-allocated, sendable future; the shape constructors and hooks return.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A value held by the runtime: an `Arc<T>` (with `T` possibly `dyn Trait`) boxed
/// behind `Any`, because `Arc<dyn Trait>` cannot be downcast directly.
/// A built value, type-erased: an `Arc<T>` (`T` possibly `dyn Trait`) boxed in an `Arc<dyn Any>`.
/// [`store`] and [`load`] are the only intended way in and out.
pub type Stored = Arc<dyn Any + Send + Sync>;

/// Erases `value` into a [`Stored`].
pub fn store<T: ?Sized + Send + Sync + 'static>(value: Arc<T>) -> Stored {
    Arc::new(value)
}

/// Recovers the `Arc<T>` that [`store`] erased, or `None` if `T` is not the stored type.
pub fn load<T: ?Sized + Send + Sync + 'static>(stored: &Stored) -> Option<Arc<T>> {
    stored.downcast_ref::<Arc<T>>().cloned()
}
