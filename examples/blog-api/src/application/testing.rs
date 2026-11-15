//! Test doubles for the repository ports. Deliberately local to this layer: the
//! application's tests must not depend on infrastructure.

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use async_trait::async_trait;
use uuid::Uuid;

use crate::domain::{
    Email, EventCursor, Page, Post, PostEvent, PostEvents, PostRepository, PostSubscription, RepositoryError,
    Sequenced, User, UserRepository,
};

#[derive(Default)]
pub(crate) struct FakeUsers {
    users: Mutex<Vec<User>>,
    fail: bool,
}

impl FakeUsers {
    /// A repository whose every call reports the store as unavailable.
    pub(crate) fn failing() -> Self {
        Self {
            fail: true,
            ..Self::default()
        }
    }

    fn check(&self) -> Result<(), RepositoryError> {
        if self.fail {
            Err(RepositoryError::Unavailable("fake outage".to_owned()))
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl UserRepository for FakeUsers {
    async fn find(&self, id: Uuid) -> Result<Option<User>, RepositoryError> {
        self.check()?;
        Ok(self.users.lock().unwrap().iter().find(|u| u.id() == id).cloned())
    }
    async fn find_by_email(&self, email: &Email) -> Result<Option<User>, RepositoryError> {
        self.check()?;
        Ok(self.users.lock().unwrap().iter().find(|u| u.email() == email).cloned())
    }
    async fn list(&self, page: Page) -> Result<Vec<User>, RepositoryError> {
        self.check()?;
        Ok(page.apply(self.users.lock().unwrap().clone()))
    }
    async fn insert(&self, user: User) -> Result<(), RepositoryError> {
        self.check()?;
        self.users.lock().unwrap().push(user);
        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct FakePosts {
    posts: Mutex<Vec<Post>>,
}

#[async_trait]
impl PostRepository for FakePosts {
    async fn find(&self, id: Uuid) -> Result<Option<Post>, RepositoryError> {
        Ok(self.posts.lock().unwrap().iter().find(|p| p.id() == id).cloned())
    }
    async fn list(&self, page: Page) -> Result<Vec<Post>, RepositoryError> {
        Ok(page.apply(self.posts.lock().unwrap().clone()))
    }
    async fn insert(&self, post: Post) -> Result<(), RepositoryError> {
        self.posts.lock().unwrap().push(post);
        Ok(())
    }
    async fn remove(&self, id: Uuid) -> Result<bool, RepositoryError> {
        let mut posts = self.posts.lock().unwrap();
        let before = posts.len();
        posts.retain(|p| p.id() != id);
        Ok(posts.len() != before)
    }
}

/// Records what was published; subscriptions are immediately over.
#[derive(Default)]
pub(crate) struct FakeEvents {
    published: Mutex<Vec<PostEvent>>,
}

impl FakeEvents {
    /// Everything published so far, oldest first.
    pub(crate) fn published(&self) -> Vec<PostEvent> {
        self.published.lock().unwrap().clone()
    }
}

struct Ended;

impl PostSubscription for Ended {
    fn next(&mut self) -> Pin<Box<dyn Future<Output = Option<Sequenced>> + Send + '_>> {
        Box::pin(async { None })
    }
}

#[async_trait]
impl PostEvents for FakeEvents {
    fn publish(&self, event: PostEvent) {
        self.published.lock().unwrap().push(event);
    }
    async fn subscribe(&self, _after: Option<EventCursor>) -> Box<dyn PostSubscription> {
        Box::new(Ended)
    }
}
