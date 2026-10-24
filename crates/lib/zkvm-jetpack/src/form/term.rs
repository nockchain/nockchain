use std::borrow::{Borrow, Cow};
use std::fmt;

/// Identifier for `term` nouns (`@tas`) that may be longer than 64 bits.
///
/// Internally we keep [`Cow`] so compile-time constants can avoid heap
/// allocation while dynamic values still get owned storage.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Term(Cow<'static, str>);

impl Term {
    /// Build a term from a static string without allocating.
    pub const fn from_static(value: &'static str) -> Self {
        Self(Cow::Borrowed(value))
    }

    /// Build a term from an owned string (allocates as needed).
    pub fn owned<S>(value: S) -> Self
    where
        S: Into<String>,
    {
        Self(Cow::Owned(value.into()))
    }

    /// Borrow the underlying string representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for Term {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for Term {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Term").field(&self.as_str()).finish()
    }
}

impl From<String> for Term {
    fn from(value: String) -> Self {
        Self::owned(value)
    }
}

impl From<&String> for Term {
    fn from(value: &String) -> Self {
        Self::owned(value.clone())
    }
}
