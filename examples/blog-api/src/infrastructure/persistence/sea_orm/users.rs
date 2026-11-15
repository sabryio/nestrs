use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::{ActiveValue, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use uuid::Uuid;

use super::entities::user;
use super::{executor, unavailable};
use crate::domain::{Email, Page, RepositoryError, User, UserName, UserRepository};
use nestrs_sea_orm::Database;

/// Adapter for [`UserRepository`] backed by SeaORM.
pub struct SeaUserRepository {
    database: Arc<Database>,
}

impl SeaUserRepository {
    /// Creates the adapter over a shared [`Database`].
    pub fn new(database: Arc<Database>) -> Self {
        Self { database }
    }
}

/// Rebuilds a domain user from a row; a row that violates the domain's rules is
/// reported as corrupt instead of being trusted.
fn to_domain(row: user::Model) -> Result<User, RepositoryError> {
    let corrupt = |reason: String| RepositoryError::Corrupt { id: row.id, reason };
    let name = UserName::parse(&row.name).map_err(|error| corrupt(error.to_string()))?;
    let email = Email::parse(&row.email).map_err(|error| corrupt(error.to_string()))?;
    Ok(User::restore(row.id, name, email))
}

#[async_trait]
impl UserRepository for SeaUserRepository {
    async fn find(&self, id: Uuid) -> Result<Option<User>, RepositoryError> {
        let db = executor(&self.database).await?;
        let row = user::Entity::find_by_id(id).one(&db).await.map_err(unavailable)?;
        row.map(to_domain).transpose()
    }

    async fn find_by_email(&self, email: &Email) -> Result<Option<User>, RepositoryError> {
        let db = executor(&self.database).await?;
        let row = user::Entity::find()
            .filter(user::Column::Email.eq(email.as_str()))
            .one(&db)
            .await
            .map_err(unavailable)?;
        row.map(to_domain).transpose()
    }

    async fn list(&self, page: Page) -> Result<Vec<User>, RepositoryError> {
        let db = executor(&self.database).await?;
        let rows = user::Entity::find()
            .order_by_asc(user::Column::Id)
            .limit(page.limit())
            .offset(page.offset())
            .all(&db)
            .await
            .map_err(unavailable)?;
        rows.into_iter().map(to_domain).collect()
    }

    async fn insert(&self, user: User) -> Result<(), RepositoryError> {
        let db = executor(&self.database).await?;
        let row = user::ActiveModel {
            id: ActiveValue::Set(user.id()),
            name: ActiveValue::Set(user.name().as_str().to_owned()),
            email: ActiveValue::Set(user.email().as_str().to_owned()),
        };
        user::Entity::insert(row).exec(&db).await.map_err(unavailable)?;
        Ok(())
    }
}
