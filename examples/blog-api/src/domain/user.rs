use uuid::Uuid;

use super::DomainError;

/// A syntactically valid, normalised (trimmed, lower-cased) email address.
///
/// The only way to get one is [`Email::parse`], so any `Email` in the system is valid.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Email(String);

impl Email {
    /// Validates and normalises `raw`.
    pub fn parse(raw: &str) -> Result<Self, DomainError> {
        let normalised = raw.trim().to_lowercase();
        let valid = match normalised.split_once('@') {
            Some((local, domain)) => {
                !local.is_empty()
                    && !domain.contains('@')
                    && domain.contains('.')
                    && !domain.starts_with('.')
                    && !domain.ends_with('.')
                    && !normalised.chars().any(char::is_whitespace)
            }
            None => false,
        };
        if valid {
            Ok(Self(normalised))
        } else {
            Err(DomainError::InvalidEmail(raw.trim().to_owned()))
        }
    }

    /// The normalised address.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A display name of 1–100 characters, trimmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserName(String);

impl UserName {
    /// Validates and trims `raw`.
    pub fn parse(raw: &str) -> Result<Self, DomainError> {
        let trimmed = raw.trim();
        if (1..=100).contains(&trimmed.chars().count()) {
            Ok(Self(trimmed.to_owned()))
        } else {
            Err(DomainError::InvalidUserName)
        }
    }

    /// The trimmed name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A registered user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    id: Uuid,
    name: UserName,
    email: Email,
}

impl User {
    /// Registers a new user with a fresh id.
    pub fn register(name: UserName, email: Email) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            email,
        }
    }

    /// Rebuilds a user from stored parts (used by repositories).
    pub fn restore(id: Uuid, name: UserName, email: Email) -> Self {
        Self { id, name, email }
    }

    /// Unique identifier.
    pub fn id(&self) -> Uuid {
        self.id
    }

    /// Display name.
    pub fn name(&self) -> &UserName {
        &self.name
    }

    /// Unique email address.
    pub fn email(&self) -> &Email {
        &self.email
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emails_are_normalised() {
        assert_eq!(Email::parse("  Ada@Example.COM ").unwrap().as_str(), "ada@example.com");
    }

    #[test]
    fn malformed_emails_are_rejected() {
        for bad in [
            "",
            "plain",
            "@example.com",
            "a@",
            "a@b",
            "a b@example.com",
            "a@@example.com",
            "a@.com",
        ] {
            assert!(Email::parse(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn names_are_trimmed_and_bounded() {
        assert_eq!(UserName::parse("  Ada ").unwrap().as_str(), "Ada");
        assert_eq!(UserName::parse("   "), Err(DomainError::InvalidUserName));
        assert_eq!(UserName::parse(&"x".repeat(101)), Err(DomainError::InvalidUserName));
        assert!(UserName::parse(&"x".repeat(100)).is_ok());
    }
}
