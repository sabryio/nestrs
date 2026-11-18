use std::collections::HashMap;
use std::sync::RwLock;

use async_trait::async_trait;
use purlin::prelude::*;
use uuid::Uuid;

use crate::domain::{Page, Post, PostRepository, RepositoryError};

/// Adapter for [`PostRepository`] that keeps posts in process memory.
#[derive(Default)]
#[injectable]
pub struct InMemoryPostRepository {
    #[inject(default)]
    posts: RwLock<HashMap<Uuid, Post>>,
}

fn poisoned<T>(_: T) -> RepositoryError {
    RepositoryError::Unavailable("in-memory store lock poisoned".to_owned())
}

#[async_trait]
impl PostRepository for InMemoryPostRepository {
    async fn find(&self, id: Uuid) -> Result<Option<Post>, RepositoryError> {
        Ok(self.posts.read().map_err(poisoned)?.get(&id).cloned())
    }

    async fn list(&self, page: Page) -> Result<Vec<Post>, RepositoryError> {
        let mut posts: Vec<Post> = self.posts.read().map_err(poisoned)?.values().cloned().collect();
        posts.sort_by_key(Post::id);
        Ok(page.apply(posts))
    }

    async fn insert(&self, post: Post) -> Result<(), RepositoryError> {
        self.posts.write().map_err(poisoned)?.insert(post.id(), post);
        Ok(())
    }

    async fn remove(&self, id: Uuid) -> Result<bool, RepositoryError> {
        Ok(self.posts.write().map_err(poisoned)?.remove(&id).is_some())
    }
}
