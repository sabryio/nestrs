//! Who may run a scheduled tick: one instance, not all of them.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use nestrs::prelude::*;
use tokio::time::Instant;

/// The lock backend could not answer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("job lock unavailable: {0}")]
pub struct LockError(pub String);

/// Port: a named, expiring, cross-instance lock.
///
/// Contract: `try_acquire` returns `true` to exactly one caller per `ttl` window across
/// every instance sharing the backend. The lock is *not* released after the job; it
/// simply expires, which also absorbs small clock and phase differences between
/// instances. A backend that is down must return `Err`: the scheduler then skips the
/// tick (a missed run is safer than a doubled one) and logs it.
#[port]
pub trait JobLock {
    /// Claims `name` for `ttl`. `Ok(false)` means another instance holds it.
    async fn try_acquire(&self, name: &str, ttl: Duration) -> Result<bool, LockError>;
}

/// A lock for a single process. Correct for one instance, and the default for tests.
#[derive(Default)]
pub struct MemoryJobLock {
    held: Mutex<HashMap<String, Instant>>,
}

impl MemoryJobLock {
    /// An empty lock table.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl JobLock for MemoryJobLock {
    async fn try_acquire(&self, name: &str, ttl: Duration) -> Result<bool, LockError> {
        let now = Instant::now();
        let mut held = self.held.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if held.get(name).is_some_and(|until| *until > now) {
            return Ok(false);
        }
        held.insert(name.to_owned(), now + ttl);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn one_claim_per_window() {
        let lock = MemoryJobLock::new();
        assert!(lock.try_acquire("a", Duration::from_secs(10)).await.unwrap());
        assert!(!lock.try_acquire("a", Duration::from_secs(10)).await.unwrap());
        assert!(
            lock.try_acquire("b", Duration::from_secs(10)).await.unwrap(),
            "names are independent"
        );
        tokio::time::advance(Duration::from_secs(11)).await;
        assert!(lock.try_acquire("a", Duration::from_secs(10)).await.unwrap());
    }
}
