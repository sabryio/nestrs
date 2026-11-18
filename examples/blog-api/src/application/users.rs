use purlin::injectable;
use uuid::Uuid;

use super::ApplicationError;
use crate::domain::{Email, Page, User, UserName, UserRepositoryPort};

/// Input of [`RegisterUser`]: raw, unvalidated values straight from the caller.
#[derive(Debug, Clone)]
pub struct RegisterUserCommand {
    /// Display name.
    pub name: String,
    /// Email address.
    pub email: String,
}

/// Use case: register a user. Emails are unique, case-insensitively.
#[injectable]
pub struct RegisterUser {
    users: UserRepositoryPort,
}

impl RegisterUser {
    /// Validates the input, rejects duplicates and stores the new user.
    pub async fn execute(&self, command: RegisterUserCommand) -> Result<User, ApplicationError> {
        let name = UserName::parse(&command.name)?;
        let email = Email::parse(&command.email)?;
        if self.users.find_by_email(&email).await?.is_some() {
            return Err(ApplicationError::EmailTaken(email.as_str().to_owned()));
        }
        let user = User::register(name, email);
        self.users.insert(user.clone()).await?;
        Ok(user)
    }
}

/// Use case: fetch one user.
#[injectable]
pub struct GetUser {
    users: UserRepositoryPort,
}

impl GetUser {
    /// Returns the user, or [`ApplicationError::UserNotFound`].
    pub async fn execute(&self, id: Uuid) -> Result<User, ApplicationError> {
        self.users.find(id).await?.ok_or(ApplicationError::UserNotFound(id))
    }
}

/// Use case: list all users.
#[injectable]
pub struct ListUsers {
    users: UserRepositoryPort,
}

impl ListUsers {
    /// One page of users in the repository's stable order.
    pub async fn execute(&self, page: Page) -> Result<Vec<User>, ApplicationError> {
        Ok(self.users.list(page).await?)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::application::testing::FakeUsers;
    use crate::domain::{DomainError, RepositoryError};

    fn command(name: &str, email: &str) -> RegisterUserCommand {
        RegisterUserCommand {
            name: name.to_owned(),
            email: email.to_owned(),
        }
    }

    #[tokio::test]
    async fn registers_and_finds_users() {
        let users = Arc::new(FakeUsers::default());
        let created = RegisterUser { users: users.clone() }
            .execute(command("Ada", "Ada@Example.com"))
            .await
            .unwrap();
        assert_eq!(created.email().as_str(), "ada@example.com");
        assert_eq!(GetUser { users }.execute(created.id()).await.unwrap(), created);
    }

    #[tokio::test]
    async fn rejects_duplicate_emails_case_insensitively() {
        let register = RegisterUser {
            users: Arc::new(FakeUsers::default()),
        };
        register.execute(command("Ada", "ada@example.com")).await.unwrap();
        let error = register.execute(command("Other", "ADA@example.com")).await.unwrap_err();
        assert_eq!(error, ApplicationError::EmailTaken("ada@example.com".to_owned()));
    }

    #[tokio::test]
    async fn invalid_input_is_a_domain_error_and_nothing_is_stored() {
        let users = Arc::new(FakeUsers::default());
        let error = RegisterUser { users: users.clone() }
            .execute(command("Ada", "nope"))
            .await
            .unwrap_err();
        assert!(matches!(error, ApplicationError::Invalid(DomainError::InvalidEmail(_))));
        assert!(ListUsers { users }.execute(Page::default()).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn unknown_ids_are_not_found() {
        let id = Uuid::new_v4();
        let error = GetUser {
            users: Arc::new(FakeUsers::default()),
        }
        .execute(id)
        .await
        .unwrap_err();
        assert_eq!(error, ApplicationError::UserNotFound(id));
    }

    #[tokio::test]
    async fn storage_failures_surface_as_repository_errors() {
        let failing = Arc::new(FakeUsers::failing());
        let error = ListUsers { users: failing }.execute(Page::default()).await.unwrap_err();
        assert!(matches!(
            error,
            ApplicationError::Repository(RepositoryError::Unavailable(_))
        ));
    }
}
