use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::{ActiveValue, EntityTrait, QueryOrder, QuerySelect};
use uuid::Uuid;

use super::database::Database;
use super::entities::post;
use super::unavailable;
use crate::domain::{Page, Post, PostBody, PostRepository, PostTitle, RepositoryError};

/// Adapter for [`PostRepository`] backed by SeaORM.
pub struct SeaPostRepository {
    database: Arc<Database>,
}

impl SeaPostRepository {
    /// Creates the adapter over a shared [`Database`].
    pub fn new(database: Arc<Database>) -> Self {
        Self { database }
    }
}

impl TryFrom<post::Model> for Post {
    type Error = RepositoryError;

    fn try_from(row: post::Model) -> Result<Self, Self::Error> {
        let id = row.id;

        let corrupt = |reason: String| RepositoryError::Corrupt { id, reason };

        let title = PostTitle::parse(&row.title).map_err(|error| corrupt(error.to_string()))?;

        let body = PostBody::parse(&row.body).map_err(|error| corrupt(error.to_string()))?;

        Ok(Post::restore(id, row.author_id, title, body))
    }
}

#[async_trait]
impl PostRepository for SeaPostRepository {
    async fn find(&self, id: Uuid) -> Result<Option<Post>, RepositoryError> {
        let row = post::Entity::find_by_id(id)
            .one(self.database.connection()?)
            .await
            .map_err(unavailable)?;
        row.map(Post::try_from).transpose()
    }

    async fn list(&self, page: Page) -> Result<Vec<Post>, RepositoryError> {
        let rows = post::Entity::find()
            .order_by_asc(post::Column::Id)
            .limit(page.limit())
            .offset(page.offset())
            .all(self.database.connection()?)
            .await
            .map_err(unavailable)?;
        rows.into_iter().map(Post::try_from).collect()
    }

    async fn insert(&self, post: Post) -> Result<(), RepositoryError> {
        let row = post::ActiveModel {
            id: ActiveValue::Set(post.id()),
            author_id: ActiveValue::Set(post.author_id()),
            title: ActiveValue::Set(post.title().as_str().to_owned()),
            body: ActiveValue::Set(post.body().as_str().to_owned()),
        };
        post::Entity::insert(row)
            .exec(self.database.connection()?)
            .await
            .map_err(unavailable)?;
        Ok(())
    }

    async fn remove(&self, id: Uuid) -> Result<bool, RepositoryError> {
        let result = post::Entity::delete_by_id(id)
            .exec(self.database.connection()?)
            .await
            .map_err(unavailable)?;
        Ok(result.rows_affected > 0)
    }
}
