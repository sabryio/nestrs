//! Identity of a value inside the dependency graph.

use std::any::TypeId;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::panic::Location;

/// Identifies one value (or one group of values) in the graph.
///
/// A key is the [`TypeId`] of the provided type, which may be unsized (`dyn Port`),
/// plus an optional name. The type name is carried only for diagnostics.
#[derive(Clone, Copy)]
pub struct Key {
    id: TypeId,
    /// `std::any::type_name::<T>`, as a pointer so the key stays small.
    ty: TypeText,
    name: Option<&'static str>,
    group: bool,
}

impl Key {
    /// The key of the unnamed value of type `T`.
    pub fn of<T: ?Sized + 'static>() -> Self {
        Self::build::<T>(None, false)
    }

    /// The key of the value of type `T` registered under `name`.
    pub fn named<T: ?Sized + 'static>(name: &'static str) -> Self {
        Self::build::<T>(Some(name), false)
    }

    pub(crate) fn group<T: ?Sized + 'static>(name: Option<&'static str>) -> Self {
        Self::build::<T>(name, true)
    }

    fn build<T: ?Sized + 'static>(name: Option<&'static str>, group: bool) -> Self {
        Self {
            id: TypeId::of::<T>(),
            ty: TypeText::Of(std::any::type_name::<T>),
            name,
            group,
        }
    }

    /// The type of an unnamed, non-group key: what interface bindings attach to.
    pub(crate) fn plain_type_id(&self) -> Option<TypeId> {
        (self.name.is_none() && !self.group).then_some(self.id)
    }

    /// Fully qualified type name, as reported by [`std::any::type_name`].
    pub fn type_name(&self) -> &'static str {
        self.ty.get()
    }

    /// The name this value is registered under, if any.
    pub fn name(&self) -> Option<&'static str> {
        self.name
    }

    /// Whether this key stands for a group of values.
    pub fn is_group(&self) -> bool {
        self.group
    }
}

/// The type name, either computed lazily from a type or supplied as text.
#[derive(Clone, Copy)]
enum TypeText {
    Of(fn() -> &'static str),
    Fixed(&'static str),
}

impl TypeText {
    fn get(self) -> &'static str {
        match self {
            Self::Of(name) => name(),
            Self::Fixed(text) => text,
        }
    }
}

impl Key {
    /// A key from its parts, for layers that hold type information at runtime (a framework
    /// translating its own registry onto fx). `type_name` is used only in messages.
    pub fn from_parts(id: TypeId, type_name: &'static str, name: Option<&'static str>, group: bool) -> Self {
        Self {
            id,
            ty: TypeText::Fixed(type_name),
            name,
            group,
        }
    }

    /// The [`TypeId`] of the provided type.
    pub fn type_id(&self) -> TypeId {
        self.id
    }
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.name == other.name && self.group == other.group
    }
}

impl Eq for Key {}

impl Hash for Key {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.name.hash(state);
        self.group.hash(state);
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.group {
            f.write_str("group of ")?;
        }
        f.write_str(&shorten(self.ty.get()))?;
        if let Some(name) = self.name {
            write!(f, " (name \"{name}\")")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Key({self})")
    }
}

/// Where a provider, decorator or invoke was declared: the function's type name and
/// the source location of the call that registered it.
#[derive(Clone, Copy, Debug)]
pub struct Origin {
    name: &'static str,
    location: &'static Location<'static>,
}

impl Origin {
    pub(crate) fn new(name: &'static str, location: &'static Location<'static>) -> Self {
        Self { name, location }
    }

    /// Fully qualified name of the registered function.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Source location where it was registered.
    pub fn location(&self) -> &'static Location<'static> {
        self.location
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "`{}` (at {}:{})",
            shorten(self.name),
            self.location.file(),
            self.location.line()
        )
    }
}

/// Drops module paths from a type name: `dyn a::b::Repo` becomes `dyn Repo`.
///
/// Closures and async blocks carry `{{closure}}` segments; those are noise too.
pub(crate) fn shorten(name: &str) -> String {
    let name = name.replace("::{{closure}}", "").replace("{{closure}}", "closure");

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
        assert_eq!(shorten("app::main::{{closure}}"), "main");
    }

    #[test]
    fn keys_distinguish_names_groups_and_trait_objects() {
        struct Concrete;
        assert_ne!(Key::of::<dyn Marker>(), Key::of::<Concrete>());
        assert_ne!(Key::of::<Concrete>(), Key::named::<Concrete>("x"));
        assert_ne!(Key::of::<Concrete>(), Key::group::<Concrete>(None));
        assert_eq!(Key::named::<Concrete>("x"), Key::named::<Concrete>("x"));
        assert_eq!(Key::named::<Concrete>("x").to_string(), "Concrete (name \"x\")");
    }
}
