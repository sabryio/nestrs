//! One name, every spelling.

/// A name that cannot become a Rust identifier / crate name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    /// Nothing was given.
    #[error("a name is required")]
    Empty,
    /// Must start with a letter.
    #[error("{0:?} must start with a letter")]
    BadStart(String),
    /// Only letters, digits, `-` and `_`.
    #[error("{0:?} may only contain letters, digits, '-' and '_'")]
    BadCharacter(String),
    /// Would be a keyword or collide with a layer directory.
    #[error("{0:?} is reserved; pick another name")]
    Reserved(String),
}

/// The spellings of one name: `invoice-line` → `invoice_line`, `InvoiceLine`, `invoice_lines`, ….
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Names {
    /// `invoice_line`: files, modules, functions.
    pub snake: String,
    /// `InvoiceLine`: types.
    pub pascal: String,
    /// `invoice_lines`: collections and file names.
    pub plural: String,
    /// `InvoiceLines`: plural types (modules, controllers).
    pub pascal_plural: String,
    /// `invoice-lines`: URL segment.
    pub route: String,
}

const RESERVED: &[&str] = &[
    "as",
    "async",
    "await",
    "break",
    "const",
    "continue",
    "crate",
    "dyn",
    "else",
    "enum",
    "extern",
    "false",
    "fn",
    "for",
    "if",
    "impl",
    "in",
    "let",
    "loop",
    "match",
    "mod",
    "move",
    "mut",
    "pub",
    "ref",
    "return",
    "self",
    "static",
    "struct",
    "super",
    "trait",
    "true",
    "type",
    "unsafe",
    "use",
    "where",
    "while",
    "domain",
    "application",
    "infrastructure",
    "presentation",
    "composition",
    "http",
    "error",
    "test",
    "tests",
];

impl Names {
    /// Parses `raw` (any of `invoice`, `invoice-line`, `invoice_line`, `InvoiceLine`);
    /// `plural` overrides the guessed plural (snake case).
    pub fn parse(raw: &str, plural: Option<&str>) -> Result<Self, NameError> {
        let snake = to_snake(raw)?;
        let plural = match plural {
            Some(custom) => to_snake(custom)?,
            None => pluralise(&snake),
        };
        let pascal = to_pascal(&snake);
        let pascal_plural = to_pascal(&plural);
        let route = plural.replace('_', "-");
        Ok(Self {
            snake,
            pascal,
            plural,
            pascal_plural,
            route,
        })
    }
}

/// Validates a crate name (`my-api` → package `my-api`, library `my_api`).
pub fn crate_names(raw: &str) -> Result<(String, String), NameError> {
    let snake = to_snake(raw)?;
    Ok((snake.replace('_', "-"), snake))
}

fn to_snake(raw: &str) -> Result<String, NameError> {
    let raw = raw.trim();
    let first = raw.chars().next().ok_or(NameError::Empty)?;
    if !first.is_ascii_alphabetic() {
        return Err(NameError::BadStart(raw.to_owned()));
    }
    if let Some(_bad) = raw
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || *c == '-' || *c == '_'))
    {
        return Err(NameError::BadCharacter(raw.to_owned()));
    }
    let mut out = String::new();
    let mut previous_lower = false;
    for c in raw.chars() {
        if c == '-' || c == '_' {
            if !out.ends_with('_') {
                out.push('_');
            }
            previous_lower = false;
        } else if c.is_ascii_uppercase() {
            if previous_lower {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
            previous_lower = false;
        } else {
            out.push(c);
            previous_lower = true;
        }
    }
    let out = out.trim_matches('_').to_owned();
    if RESERVED.contains(&out.as_str()) {
        return Err(NameError::Reserved(out));
    }
    Ok(out)
}

fn to_pascal(snake: &str) -> String {
    snake
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + chars.as_str()
            })
        })
        .collect()
}

fn pluralise(snake: &str) -> String {
    let is_vowel = |c: char| "aeiou".contains(c);
    if snake.ends_with('s') || snake.ends_with('x') || snake.ends_with("ch") || snake.ends_with("sh") {
        format!("{snake}es")
    } else if let Some(stem) = snake.strip_suffix('y') {
        if stem.chars().last().is_some_and(|c| !is_vowel(c)) {
            format!("{stem}ies")
        } else {
            format!("{snake}s")
        }
    } else {
        format!("{snake}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_spelling_follows_from_any_input_style() {
        for raw in ["invoice-line", "invoice_line", "InvoiceLine", "invoiceLine"] {
            let names = Names::parse(raw, None).unwrap();
            assert_eq!(names.snake, "invoice_line", "{raw}");
            assert_eq!(names.pascal, "InvoiceLine");
            assert_eq!(names.plural, "invoice_lines");
            assert_eq!(names.pascal_plural, "InvoiceLines");
            assert_eq!(names.route, "invoice-lines");
        }
    }

    #[test]
    fn plurals_follow_english_endings_and_can_be_overridden() {
        let plural = |name: &str| Names::parse(name, None).unwrap().plural;
        assert_eq!(plural("category"), "categories");
        assert_eq!(plural("day"), "days");
        assert_eq!(plural("box"), "boxes");
        assert_eq!(plural("address"), "addresses");
        assert_eq!(Names::parse("person", Some("people")).unwrap().plural, "people");
    }

    #[test]
    fn bad_names_are_refused_with_a_reason() {
        assert_eq!(Names::parse("", None), Err(NameError::Empty));
        assert!(matches!(Names::parse("9lives", None), Err(NameError::BadStart(_))));
        assert!(matches!(Names::parse("a b", None), Err(NameError::BadCharacter(_))));
        assert!(matches!(Names::parse("domain", None), Err(NameError::Reserved(_))));
        assert!(matches!(Names::parse("type", None), Err(NameError::Reserved(_))));
    }
}
