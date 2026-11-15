use std::collections::HashMap;
use std::sync::RwLock;

use async_trait::async_trait;
use nestrs::prelude::*;
use uuid::Uuid;

use crate::domain::{Email, Page, RepositoryError, User, UserRepository};

/// Adapter for [`UserRepository`] that keeps users in process memory.
#[derive(Default)]
#[injectable]
pub struct InMemoryUserRepository {
    #[inject(default)]
    users: RwLock<HashMap<Uuid, User>>,
}

fn poisoned<T>(_: T) -> RepositoryError {
    RepositoryError::Unavailable("in-memory store lock poisoned".to_owned())
}

#[async_trait]
impl UserRepository for InMemoryUserRepository {
    async fn find(&self, id: Uuid) -> Result<Option<User>, RepositoryError> {
        Ok(self.users.read().map_err(poisoned)?.get(&id).cloned())
    }

    async fn find_by_email(&self, email: &Email) -> Result<Option<User>, RepositoryError> {
        let users = self.users.read().map_err(poisoned)?;
        Ok(users.values().find(|user| user.email() == email).cloned())
    }

    async fn list(&self, page: Page) -> Result<Vec<User>, RepositoryError> {
        let mut users: Vec<User> = self.users.read().map_err(poisoned)?.values().cloned().collect();
        users.sort_by_key(User::id);
        Ok(page.apply(users))
    }

    async fn insert(&self, user: User) -> Result<(), RepositoryError> {
        self.users.write().map_err(poisoned)?.insert(user.id(), user);
        Ok(())
    }
}
