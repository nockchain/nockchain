use nockvm::noun::{Noun, NounAllocator, D, T};

use crate::utils::make_tas;

/// Standardized wire format for kernel interaction.
pub trait Wire: Sized {
    /// Protocol version
    const VERSION: u64;

    /// Driver/Module identifier
    const SOURCE: &'static str;

    /// Get wire for this driver. Specific implementations can add more tags to the wire as they wish but the default is just [`Self::SOURCE`] and [`Self::VERSION`].
    fn to_wire(&self) -> WireRepr {
        WireRepr::no_tags(Self::SOURCE, Self::VERSION)
    }
}

/// Converts a wire to a Noun by allocating it on the kernel's stack.
pub(crate) fn wire_to_noun<A: NounAllocator>(stack: &mut A, wire: &WireRepr) -> Noun {
    let source_atom = make_tas(stack, wire.source);
    let version_atom: Noun = D(wire.version);
    if wire.tags.is_empty() {
        T(stack, &[source_atom.as_noun(), version_atom, D(0)])
    } else {
        let mut wire_noun = Vec::with_capacity(wire.tags.len() + 3);
        wire_noun.push(make_tas(stack, wire.source).as_noun());
        wire_noun.push(D(wire.version));
        for tag in &wire.tags {
            wire_noun.push(tag.as_noun(stack));
        }
        wire_noun.push(D(0));

        T(stack, &wire_noun)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireTag {
    Direct(u64),
    String(String),
}

impl WireTag {
    pub fn as_noun<A: NounAllocator>(&self, stack: &mut A) -> Noun {
        match self {
            WireTag::Direct(d) => D(*d),
            WireTag::String(s) => make_tas(stack, s).as_noun(),
        }
    }
}

impl std::fmt::Display for WireTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WireTag::Direct(d) => write!(f, "{}", d),
            WireTag::String(s) => write!(f, "{}", s),
        }
    }
}

impl From<u8> for WireTag {
    fn from(d: u8) -> Self {
        WireTag::Direct(d as u64)
    }
}
impl From<String> for WireTag {
    fn from(s: String) -> Self {
        WireTag::String(s)
    }
}

impl From<u64> for WireTag {
    fn from(d: u64) -> Self {
        WireTag::Direct(d)
    }
}

impl From<&u64> for WireTag {
    fn from(d: &u64) -> Self {
        WireTag::Direct(*d)
    }
}

impl From<&str> for WireTag {
    fn from(s: &str) -> Self {
        WireTag::String(s.to_string())
    }
}

/// WireRepr is intended to make the default scenario (no custom tags beyond source and version) not allocate on the heap up-front.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireRepr {
    pub source: &'static str,
    pub version: u64,
    pub tags: Vec<WireTag>,
}

impl WireRepr {
    pub fn new(source: &'static str, version: u64, tags: Vec<WireTag>) -> Self {
        WireRepr {
            source,
            version,
            tags,
        }
    }
    pub fn no_tags(source: &'static str, version: u64) -> Self {
        WireRepr {
            source,
            version,
            tags: Vec::new(),
        }
    }
    pub fn tags_as_csv(&self) -> String {
        let mut tags = Vec::with_capacity(self.tags.len() + 2);
        tags.push(self.source.to_string());
        tags.push(self.version.to_string());
        for tag in &self.tags {
            match tag {
                WireTag::Direct(d) => tags.push(d.to_string()),
                WireTag::String(s) => tags.push(s.clone()),
            }
        }
        tags.join(",")
    }
}

/// System wire to use when no other wire is specified
pub struct SystemWire;

impl Wire for SystemWire {
    const VERSION: u64 = 1;
    const SOURCE: &'static str = "sys";
}
