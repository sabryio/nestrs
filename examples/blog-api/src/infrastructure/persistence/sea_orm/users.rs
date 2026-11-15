use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::{ActiveValue, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use uuid::Uuid;

use super::database::Database;
use super::entities::user;
use super::unavailable;
use crate::domain::{Email, Page, RepositoryError, User, UserName, UserRepository};

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

impl TryFrom<user::Model> for User {
    type Error = RepositoryError;

    /// Rebuilds a domain user from a row; a row that violates the domain's rules is
    /// reported as corrupt instead of being trusted.
    fn try_from(row: user::Model) -> Result<Self, Self::Error> {
        let corrupt = |reason: String| RepositoryError::Corrupt { id: row.id, reason };

        let name = UserName::parse(&row.name).map_err(|error| corrupt(error.to_string()))?;

        let email = Email::parse(&row.email).map_err(|error| corrupt(error.to_string()))?;

        Ok(User::restore(row.id, name, email))
    }
}

#[async_trait]
impl UserRepository for SeaUserRepository {
    async fn find(&self, id: Uuid) -> Result<Option<User>, RepositoryError> {
        let row = user::Entity::find_by_id(id)
            .one(self.database.connection()?)
            .await
            .map_err(unavailable)?;
        row.map(User::try_from).transpose()
    }

    async fn find_by_email(&self, email: &Email) -> Result<Option<User>, RepositoryError> {
        let row = user::Entity::find()
            .filter(user::Column::Email.eq(email.as_str()))
            .one(self.database.connection()?)
            .await
            .map_err(unavailable)?;
        row.map(User::try_from).transpose()
    }

    async fn list(&self, page: Page) -> Result<Vec<User>, RepositoryError> {
        let rows = user::Entity::find()
            .order_by_asc(user::Column::Id)
            .limit(page.limit())
            .offset(page.offset())
            .all(self.database.connection()?)
            .await
            .map_err(unavailable)?;
        rows.into_iter().map(User::try_from).collect()
    }

    async fn insert(&self, user: User) -> Result<(), RepositoryError> {
        let row = user::ActiveModel {
            id: ActiveValue::Set(user.id()),
            name: ActiveValue::Set(user.name().as_str().to_owned()),
            email: ActiveValue::Set(user.email().as_str().to_owned()),
        };
        user::Entity::insert(row)
            .exec(self.database.connection()?)
            .await
            .map_err(unavailable)?;
        Ok(())
    }
}
