//! The domain: pure Rust, no HTTP, no framework types besides the port attribute.

use std::sync::Arc;

use serde::Serialize;

/// A source of time. A port, so tests can pin it.
#[fx::port]
pub trait Clock {
    /// Seconds since the epoch.
    fn now(&self) -> u64;
}

/// Where greetings are kept. A port: the adapter decides how.
#[fx::port]
pub trait GreetingStore {
    /// Appends a greeting.
    fn save(&self, greeting: Greeting);
    /// Every greeting, oldest first.
    fn all(&self) -> Vec<Greeting>;
}

/// A recorded greeting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Greeting {
    /// Who was greeted.
    pub name: String,
    /// The message.
    pub message: String,
    /// When it was said, from the [`Clock`].
    pub at: u64,
}

/// Why a greeting was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GreetError {
    /// The name was empty or only whitespace.
    EmptyName,
}

impl std::fmt::Display for GreetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyName => f.write_str("name must not be empty"),
        }
    }
}

impl std::error::Error for GreetError {}

/// The use case: greet someone and remember it.
pub struct GreetingService {
    clock: Arc<ClockPort>,
    store: Arc<GreetingStorePort>,
}

#[fx::component]
impl GreetingService {
    /// Wires the service. Both arguments are ports.
    #[fx::constructor]
    pub fn new(clock: Arc<ClockPort>, store: Arc<GreetingStorePort>) -> Self {
        Self { clock, store }
    }

    /// Greets `name` and records it.
    ///
    /// # Errors
    ///
    /// [`GreetError::EmptyName`] if `name` is blank.
    pub fn greet(&self, name: &str) -> Result<Greeting, GreetError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(GreetError::EmptyName);
        }
        let greeting = Greeting {
            name: name.to_owned(),
            message: format!("Hello, {name}!"),
            at: self.clock.now(),
        };
        self.store.save(greeting.clone());
        Ok(greeting)
    }

    /// Every greeting so far.
    pub fn history(&self) -> Vec<Greeting> {
        self.store.all()
    }
}
