//! serde's naming rules, so wire names match what the server really sends.

/// A `rename_all` rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameRule {
    Lower,
    Upper,
    Pascal,
    Camel,
    Snake,
    ScreamingSnake,
    Kebab,
    ScreamingKebab,
}

impl RenameRule {
    pub fn parse(rule: &str) -> Option<Self> {
        Some(match rule {
            "lowercase" => Self::Lower,
            "UPPERCASE" => Self::Upper,
            "PascalCase" => Self::Pascal,
            "camelCase" => Self::Camel,
            "snake_case" => Self::Snake,
            "SCREAMING_SNAKE_CASE" => Self::ScreamingSnake,
            "kebab-case" => Self::Kebab,
            "SCREAMING-KEBAB-CASE" => Self::ScreamingKebab,
            _ => return None,
        })
    }

    /// Applies the rule to a Rust identifier (`snake_case` for fields, `PascalCase` for variants).
    pub fn apply(self, ident: &str) -> String {
        let words = words(ident);
        let join =
            |separator: &str, f: fn(&str) -> String| words.iter().map(|w| f(w)).collect::<Vec<_>>().join(separator);
        match self {
            Self::Lower => words.concat().to_lowercase(),
            Self::Upper => words.concat().to_uppercase(),
            Self::Pascal => join("", capitalise),
            Self::Camel => {
                let pascal = join("", capitalise);
                let mut chars = pascal.chars();
                chars.next().map_or_else(String::new, |first| {
                    first.to_lowercase().collect::<String>() + chars.as_str()
                })
            }
            Self::Snake => join("_", str::to_lowercase),
            Self::ScreamingSnake => join("_", str::to_uppercase),
            Self::Kebab => join("-", str::to_lowercase),
            Self::ScreamingKebab => join("-", str::to_uppercase),
        }
    }
}

fn capitalise(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
    })
}

/// Splits `snake_case` and `PascalCase` identifiers into words.
fn words(ident: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    for part in ident.split('_').filter(|part| !part.is_empty()) {
        let mut current = String::new();
        for ch in part.chars() {
            if ch.is_uppercase() && !current.is_empty() && current.chars().last().is_some_and(char::is_lowercase) {
                words.push(std::mem::take(&mut current));
            }
            current.push(ch);
        }
        words.push(current);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_and_variants_follow_serdes_rules() {
        assert_eq!(RenameRule::Camel.apply("author_id"), "authorId");
        assert_eq!(RenameRule::Snake.apply("PostPublished"), "post_published");
        assert_eq!(RenameRule::ScreamingSnake.apply("PostPublished"), "POST_PUBLISHED");
        assert_eq!(RenameRule::Kebab.apply("author_id"), "author-id");
        assert_eq!(RenameRule::Pascal.apply("author_id"), "AuthorId");
        assert_eq!(RenameRule::Lower.apply("Published"), "published");
        assert_eq!(RenameRule::Upper.apply("author_id"), "AUTHORID");
    }

    #[test]
    fn unknown_rules_are_not_guessed() {
        assert_eq!(RenameRule::parse("sNaKe"), None);
    }
}
