use std::any::TypeId;
use std::fmt;
use std::hash::{Hash, Hasher};

/// Identity of a provider inside the container.
///
/// A token wraps the [`TypeId`] of the provided type, which may be unsized: the
/// token for `dyn UserRepository` is distinct from the token of any concrete
/// implementation. The type name is carried along purely for diagnostics and does
/// not participate in equality or hashing.
#[derive(Clone, Copy)]
pub struct Token {
    id: TypeId,
    name: &'static str,
}

impl Token {
    /// Creates the token for `T`.
    pub fn of<T: ?Sized + 'static>() -> Self {
        Self {
            id: TypeId::of::<T>(),
            name: std::any::type_name::<T>(),
        }
    }

    /// The type id this token stands for.
    pub fn id(&self) -> TypeId {
        self.id
    }

    /// Fully qualified type name, as reported by [`std::any::type_name`].
    pub fn name(&self) -> &'static str {
        self.name
    }
}

impl PartialEq for Token {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Token {}

impl Hash for Token {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&shorten(self.name))
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Token({})", shorten(self.name))
    }
}

/// Drops module paths from a type name: `dyn a::b::Repo` becomes `dyn Repo`.
///
/// Types declared inside functions or async blocks carry `{{closure}}` path
/// segments in their name; those are noise in diagnostics and are dropped too.
fn shorten(name: &str) -> String {
    let name = name.replace("{{closure}}", "");
    let name = name.as_str();

    fn flush(chunk: &mut String, out: &mut String) {
        match chunk.rfind("::") {
            Some(index) => out.push_str(&chunk[index + 2..]),
            None => out.push_str(chunk),
        }
        chunk.clear();
    }

    let mut out = String::with_capacity(name.len());
    let mut chunk = String::new();
    for ch in name.chars() {
        if ch.is_alphanumeric() || ch == '_' || ch == ':' {
            chunk.push(ch);
        } else {
            flush(&mut chunk, &mut out);
            out.push(ch);
        }
    }
    flush(&mut chunk, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    trait Marker {}

    #[test]
    fn shortens_paths_but_keeps_structure() {
        assert_eq!(shorten("a::b::Repo"), "Repo");
        assert_eq!(shorten("dyn a::Repo"), "dyn Repo");
        assert_eq!(shorten("alloc::vec::Vec<a::b::Item>"), "Vec<Item>");
    }

    #[test]
    fn tokens_distinguish_trait_objects_from_concrete_types() {
        struct Concrete;
        assert_ne!(Token::of::<dyn Marker>(), Token::of::<Concrete>());
        assert_eq!(Token::of::<Concrete>(), Token::of::<Concrete>());
    }
}
