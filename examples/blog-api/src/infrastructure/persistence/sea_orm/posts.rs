use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::{ActiveValue, EntityTrait, QueryOrder, QuerySelect};
use uuid::Uuid;

use super::entities::post;
use super::{executor, unavailable};
use crate::domain::{Page, Post, PostBody, PostRepository, PostTitle, RepositoryError};
use purlin_sea_orm::Database;

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

fn to_domain(row: post::Model) -> Result<Post, RepositoryError> {
    let corrupt = |reason: String| RepositoryError::Corrupt { id: row.id, reason };
    let title = PostTitle::parse(&row.title).map_err(|error| corrupt(error.to_string()))?;
    let body = PostBody::parse(&row.body).map_err(|error| corrupt(error.to_string()))?;
    Ok(Post::restore(row.id, row.author_id, title, body))
}

#[async_trait]
impl PostRepository for SeaPostRepository {
    async fn find(&self, id: Uuid) -> Result<Option<Post>, RepositoryError> {
        let db = executor(&self.database).await?;
        let row = post::Entity::find_by_id(id).one(&db).await.map_err(unavailable)?;
        row.map(to_domain).transpose()
    }

    async fn list(&self, page: Page) -> Result<Vec<Post>, RepositoryError> {
        let db = executor(&self.database).await?;
        let rows = post::Entity::find()
            .order_by_asc(post::Column::Id)
            .limit(page.limit())
            .offset(page.offset())
            .all(&db)
            .await
            .map_err(unavailable)?;
        rows.into_iter().map(to_domain).collect()
    }

    async fn insert(&self, post: Post) -> Result<(), RepositoryError> {
        let db = executor(&self.database).await?;
        let row = post::ActiveModel {
            id: ActiveValue::Set(post.id()),
            author_id: ActiveValue::Set(post.author_id()),
            title: ActiveValue::Set(post.title().as_str().to_owned()),
            body: ActiveValue::Set(post.body().as_str().to_owned()),
        };
        post::Entity::insert(row).exec(&db).await.map_err(unavailable)?;
        Ok(())
    }

    async fn remove(&self, id: Uuid) -> Result<bool, RepositoryError> {
        let db = executor(&self.database).await?;
        let result = post::Entity::delete_by_id(id).exec(&db).await.map_err(unavailable)?;
        Ok(result.rows_affected > 0)
    }
}
