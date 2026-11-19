//! Adapters for the domain's ports.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::domain::{Clock, Greeting, GreetingStore};

/// The wall clock.
#[derive(Default)]
pub struct SystemClock;

#[fx::component]
impl SystemClock {
    /// Constructor.
    #[fx::constructor]
    pub fn new() -> Self {
        Self
    }
}

#[fx::bind]
impl Clock for SystemClock {
    fn now(&self) -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
    }
}

/// Keeps greetings in memory.
#[derive(Default)]
pub struct InMemoryStore(Mutex<Vec<Greeting>>);

#[fx::component]
impl InMemoryStore {
    /// Constructor.
    #[fx::constructor]
    pub fn new() -> Self {
        Self::default()
    }
}

#[fx::bind]
impl GreetingStore for InMemoryStore {
    fn save(&self, greeting: Greeting) {
        // A poisoned lock only means another request panicked; the data is still sound.
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(greeting);
    }

    fn all(&self) -> Vec<Greeting> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}
