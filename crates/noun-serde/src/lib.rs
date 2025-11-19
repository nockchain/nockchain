use std::collections::{BTreeMap, HashMap, HashSet};
use std::str;

use bytes::Bytes;
use ibig::UBig;
pub mod wallet;

use nockvm::ext::{make_tas, AtomExt};
use nockvm::jets::util::BAIL_FAIL;
use nockvm::jets::JetErr;
#[allow(unused_imports)]
#[allow(unused_imports)]
use nockvm::noun::{Atom, FullDebugCell, Noun, NounAllocator, Slots, D, T};
use nockvm::noun::{NO, YES};
pub use noun_serde_derive::{NounDecode, NounEncode};
use tracing::trace;

pub mod prelude {
    pub use super::{NounDecode, NounEncode, NounSerdeDecodeExt, NounSerdeEncodeExt};
}

// Trait extensions for Noun
pub trait NounSerdeDecodeExt {
    fn decode<T: NounDecode>(&self) -> Result<T, NounDecodeError>;
}

impl NounSerdeDecodeExt for nockvm::noun::Noun {
    fn decode<T: NounDecode>(&self) -> Result<T, NounDecodeError> {
        T::from_noun(self)
    }
}

pub trait NounSerdeEncodeExt: NounEncode {
    fn encode<A: NounAllocator>(&self, allocator: &mut A) -> nockvm::noun::Noun {
        self.to_noun(allocator)
    }
}

impl<T: NounEncode + ?Sized> NounSerdeEncodeExt for T {}

/// Trait for types that can be encoded as a Noun
pub trait NounEncode {
    /// Encode this value as a Noun
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun;
}

/// Trait for types that can be decoded from a Noun
pub trait NounDecode: Sized {
    /// Try to decode this value from a Noun
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError>;
}

/// Error that can occur during Noun decoding
#[derive(Debug, thiserror::Error)]
pub enum NounDecodeError {
    #[error("Expected atom, found cell")]
    ExpectedAtom,

    #[error("Expected cell, found atom")]
    ExpectedCell,

    #[error("Failed to decode field {0}: {1}")]
    FieldError(String, String),

    #[error("Invalid enum variant")]
    InvalidEnumVariant,

    #[error("Invalid enum data")]
    InvalidEnumData,

    #[error("Invalid tag")]
    InvalidTag,

    #[error("Custom error: {0}")]
    Custom(String),

    #[error("Failed to decode Mary")]
    MaryDecodeError,

    #[error("Failed to decode FPoly")]
    FPolyDecodeError,

    #[error("Failed to decode Constraints")]
    ConstraintsDecodeError,
}

impl From<NounDecodeError> for JetErr {
    fn from(_err: NounDecodeError) -> Self {
        BAIL_FAIL
    }
}

impl From<nockvm::noun::Error> for NounDecodeError {
    fn from(err: nockvm::noun::Error) -> Self {
        NounDecodeError::Custom(err.to_string())
    }
}

impl From<str::Utf8Error> for NounDecodeError {
    fn from(err: str::Utf8Error) -> Self {
        NounDecodeError::Custom(err.to_string())
    }
}

// Base no-nop implementations for Noun
impl NounEncode for Noun {
    fn to_noun<A: NounAllocator>(&self, _allocator: &mut A) -> Noun {
        *self
    }
}

impl NounDecode for Noun {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        Ok(*noun)
    }
}

// Implementations for primitive types
impl NounEncode for u64 {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        // Use Atom::new which will create direct or indirect atom as needed
        Atom::new(allocator, *self).as_noun()
    }
}

impl NounDecode for u64 {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        match noun.as_atom() {
            Ok(atom) => atom
                .as_u64()
                .map_err(|_| NounDecodeError::Custom("Atom too large for u64".into())),
            Err(_) => Err(NounDecodeError::ExpectedAtom),
        }
    }
}

impl NounEncode for u32 {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        // Convert to u64 and use Atom::new
        Atom::new(allocator, *self as u64).as_noun()
    }
}

impl NounEncode for UBig {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        Atom::from_ubig(allocator, self).as_noun()
    }
}

impl NounDecode for u32 {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        match noun.as_atom() {
            Ok(atom) => atom
                .as_u64()
                .map(|x| x as u32)
                .map_err(|_| NounDecodeError::Custom("Atom too large for u32".into())),
            Err(_) => Err(NounDecodeError::ExpectedAtom),
        }
    }
}

impl NounEncode for String {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        use nockvm::ext::make_tas;
        make_tas(allocator, self).as_noun()
    }
}

impl NounDecode for String {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        match noun.as_atom() {
            Ok(atom) => atom
                .into_string()
                .map_err(|err| NounDecodeError::Custom(format!("Invalid string atom: {:?}", err))),
            Err(_) => Err(NounDecodeError::ExpectedAtom),
        }
    }
}

impl<X: NounDecode, Y: NounDecode> NounDecode for (X, Y) {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let a = X::from_noun(&cell.slot(2)?)?;
        let b = Y::from_noun(&cell.slot(3)?)?;
        Ok((a, b))
    }
}

impl<X: NounEncode, Y: NounEncode> NounEncode for (X, Y) {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let (a, b) = self;
        let a_noun = a.to_noun(allocator);
        let b_noun = b.to_noun(allocator);
        T(allocator, &[a_noun, b_noun])
    }
}

impl<X: NounDecode, Y: NounDecode, Z: NounDecode> NounDecode for (X, Y, Z) {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let a = X::from_noun(&cell.slot(2)?)?;
        let b = Y::from_noun(&cell.slot(6)?)?;
        let c = Z::from_noun(&cell.slot(7)?)?;
        Ok((a, b, c))
    }
}

impl<X: NounEncode, Y: NounEncode, Z: NounEncode> NounEncode for (X, Y, Z) {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let (a, b, c) = self;
        let a_noun = a.to_noun(allocator);
        let b_noun = b.to_noun(allocator);
        let c_noun = c.to_noun(allocator);
        let bc = T(allocator, &[b_noun, c_noun]);
        T(allocator, &[a_noun, bc])
    }
}

impl<W: NounDecode, X: NounDecode, Y: NounDecode, Z: NounDecode> NounDecode for (W, X, Y, Z) {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let a = W::from_noun(&cell.slot(2)?)?;
        let b = X::from_noun(&cell.slot(6)?)?;
        let c = Y::from_noun(&cell.slot(14)?)?;
        let d = Z::from_noun(&cell.slot(15)?)?;
        Ok((a, b, c, d))
    }
}

impl<W: NounEncode, X: NounEncode, Y: NounEncode, Z: NounEncode> NounEncode for (W, X, Y, Z) {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let (a, b, c, d) = self;
        let a_noun = a.to_noun(allocator);
        let b_noun = b.to_noun(allocator);
        let c_noun = c.to_noun(allocator);
        let d_noun = d.to_noun(allocator);
        let cd = T(allocator, &[c_noun, d_noun]);
        let bcd = T(allocator, &[b_noun, cd]);
        T(allocator, &[a_noun, bcd])
    }
}

impl<V: NounDecode, W: NounDecode, X: NounDecode, Y: NounDecode, Z: NounDecode> NounDecode
    for (V, W, X, Y, Z)
{
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let a = V::from_noun(&cell.slot(2)?)?;
        let b = W::from_noun(&cell.slot(6)?)?;
        let c = X::from_noun(&cell.slot(14)?)?;
        let d = Y::from_noun(&cell.slot(30)?)?;
        let e = Z::from_noun(&cell.slot(31)?)?;
        Ok((a, b, c, d, e))
    }
}

impl<V: NounEncode, W: NounEncode, X: NounEncode, Y: NounEncode, Z: NounEncode> NounEncode
    for (V, W, X, Y, Z)
{
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let (a, b, c, d, e) = self;
        let a_noun = a.to_noun(allocator);
        let b_noun = b.to_noun(allocator);
        let c_noun = c.to_noun(allocator);
        let d_noun = d.to_noun(allocator);
        let e_noun = e.to_noun(allocator);
        let de = T(allocator, &[d_noun, e_noun]);
        let cde = T(allocator, &[c_noun, de]);
        let bcde = T(allocator, &[b_noun, cde]);
        T(allocator, &[a_noun, bcde])
    }
}

impl<U: NounDecode, V: NounDecode, W: NounDecode, X: NounDecode, Y: NounDecode, Z: NounDecode>
    NounDecode for (U, V, W, X, Y, Z)
{
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let a = U::from_noun(&cell.slot(2)?)?;
        let b = V::from_noun(&cell.slot(6)?)?;
        let c = W::from_noun(&cell.slot(14)?)?;
        let d = X::from_noun(&cell.slot(30)?)?;
        let e = Y::from_noun(&cell.slot(62)?)?;
        let f = Z::from_noun(&cell.slot(63)?)?;
        Ok((a, b, c, d, e, f))
    }
}

impl<U: NounEncode, V: NounEncode, W: NounEncode, X: NounEncode, Y: NounEncode, Z: NounEncode>
    NounEncode for (U, V, W, X, Y, Z)
{
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let (a, b, c, d, e, f) = self;
        let a_noun = a.to_noun(allocator);
        let b_noun = b.to_noun(allocator);
        let c_noun = c.to_noun(allocator);
        let d_noun = d.to_noun(allocator);
        let e_noun = e.to_noun(allocator);
        let f_noun = f.to_noun(allocator);
        let ef = T(allocator, &[e_noun, f_noun]);
        let def = T(allocator, &[d_noun, ef]);
        let cdef = T(allocator, &[c_noun, def]);
        let bcdef = T(allocator, &[b_noun, cdef]);
        T(allocator, &[a_noun, bcdef])
    }
}

impl NounEncode for bool {
    #[allow(unused_variables)]
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        match self {
            true => YES,
            false => NO,
        }
    }
}

impl NounDecode for bool {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        trace!("Decoding bool from noun: {:?}", noun);
        match noun.as_atom() {
            Ok(atom) => {
                trace!("Successfully decoded as atom: {:?}", atom);
                match atom.as_u64() {
                    Ok(0) => {
                        trace!("Decoded as 0 -> true (%.y)");
                        Ok(true)
                    }
                    Ok(1) => {
                        trace!("Decoded as 1 -> false (%.n)");
                        Ok(false)
                    }
                    other => {
                        trace!("Invalid boolean value: {:?}", other);
                        Err(NounDecodeError::Custom("Invalid boolean value".into()))
                    }
                }
            }
            Err(e) => {
                trace!("Failed to decode as atom: {:?}", e);
                Err(NounDecodeError::ExpectedAtom)
            }
        }
    }
}
impl<T: NounEncode> NounEncode for Option<T> {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        match self {
            Some(value) => {
                let value_noun = value.to_noun(allocator);
                T(allocator, &[D(0), value_noun])
            }
            None => D(0),
        }
    }
}

impl<T: NounDecode> NounDecode for Option<T> {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        // First check if it's an atom 0 (None)
        if let Ok(atom) = noun.as_atom() {
            match atom.as_u64() {
                Ok(0) => {
                    trace!("Found ~ (0), returning None");
                    return Ok(None);
                }
                _ => return Err(NounDecodeError::Custom("Invalid Option encoding".into())),
            }
        }

        // Otherwise it must be a cell [~ value]
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let head = cell
            .head()
            .as_atom()
            .map_err(|_| NounDecodeError::ExpectedAtom)?;

        if head.as_u64()? != 0 {
            return Err(NounDecodeError::Custom(
                "Invalid Option encoding - expected ~".into(),
            ));
        }

        let value = T::from_noun(&cell.tail())?;
        Ok(Some(value))
    }
}

impl<T: NounEncode> NounEncode for Vec<T> {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        self.iter().rev().fold(D(0), |acc, item| {
            let item_noun = item.to_noun(allocator);
            T(allocator, &[item_noun, acc])
        })
    }
}

impl<T: NounDecode> NounDecode for Vec<T> {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let mut result = Vec::new();
        let mut current = noun;
        #[allow(unused_assignments)]
        let mut current_tail = None;

        while let Ok(cell) = current.as_cell() {
            let item = T::from_noun(&cell.head())?;
            result.push(item);
            current_tail = Some(cell.tail());
            current = current_tail.as_ref().unwrap();
        }

        if let Ok(atom) = current.as_atom() {
            match atom.as_u64() {
                Ok(0) => (),
                // _ => return Err(NounDecodeError::Custom("Invalid list termination".into())),
                _ => panic!("failure"),
            }
        } else {
            return Err(NounDecodeError::ExpectedAtom);
        }

        Ok(result)
    }
}

impl NounEncode for Vec<u8> {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        self.iter()
            .map(|&x| x as u64)
            .collect::<Vec<_>>()
            .to_noun(allocator)
    }
}

impl NounDecode for Vec<u8> {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let nums = Vec::<u64>::from_noun(noun)?;
        Ok(nums.into_iter().map(|x| x as u8).collect())
    }
}

/// Implements noun encoding for HashMap types.
///
/// The encoding follows a right-branching binary tree structure where each node contains
/// a key-value pair and points to the rest of the pairs. The structure looks like:
/// ```text
/// [
///   [k1 v1]                   // head: first pair
///   [                         // tail: rest of pairs
///     [k2 v2]                 // head: second pair
///     [                       // tail: rest of pairs
///       [k3 v3]              // head: third pair
///       0                    // tail: terminator (atom 0)
///     ]
///   ]
/// ]
/// ```
///
/// # Type Parameters
///
/// * `K`: Key type that implements NounEncode + Hash + Eq
/// * `V`: Value type that implements NounEncode
///
/// # Examples
///
/// ```rust
/// # use std::collections::HashMap;
/// # use noun_serde::{NounEncode, NounDecode};
/// # use nockvm::mem::NockStack;
/// let mut map = HashMap::new();
/// map.insert("key1".to_string(), 42u64);
/// map.insert("key2".to_string(), 43u64);
///
/// let mut stack = NockStack::new(8 << 10 << 10, 0);
/// let encoded = map.to_noun(&mut stack);
/// let decoded = HashMap::<String, u64>::from_noun(&encoded).unwrap();
/// assert_eq!(map, decoded);
/// ```
impl<K: NounEncode, V: NounEncode> NounEncode for HashMap<K, V>
where
    K: std::hash::Hash + Eq,
{
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        if self.is_empty() {
            return D(0);
        }

        let entries: Vec<_> = self.iter().collect();

        fn build_tree<A: NounAllocator>(allocator: &mut A, entries: &[(&Noun, &Noun)]) -> Noun {
            if entries.is_empty() {
                return D(0);
            }

            let mid = entries.len() / 2;
            let (k, v) = &entries[mid];

            let node = T(allocator, &[(*k).clone(), (*v).clone()]);
            let left = build_tree(allocator, &entries[..mid]);
            let right = build_tree(allocator, &entries[mid + 1..]);

            T(allocator, &[node, left, right])
        }

        let encoded: Vec<_> = entries
            .iter()
            .map(|(k, v)| (k.to_noun(allocator), v.to_noun(allocator)))
            .collect();

        let refs: Vec<_> = encoded.iter().map(|(k, v)| (k, v)).collect();
        build_tree(allocator, &refs)
    }
}

/// Implements noun decoding for HashMap types.
///
/// The decoding process walks through the binary tree structure created by the encoder,
/// extracting key-value pairs until it hits the terminator atom (0).
///
/// # Type Parameters
///
/// * `K`: Key type that implements NounDecode + Hash + Eq
/// * `V`: Value type that implements NounDecode
///
/// # Errors
///
/// Returns `NounDecodeError` if:
/// - The noun structure doesn't match the expected binary tree format
/// - A key-value pair cell is malformed
/// - The list isn't properly terminated with atom 0
/// - Any key or value fails to decode
///
/// # Implementation Notes
///
/// The decoding process:
/// 1. Starts with an empty HashMap
/// 2. For each cell in the chain:
///    - Head contains a [key value] cell
///    - Tail is a pair of `(tree [k v])`  (could be terminator)
/// 3. Continues until it hits the terminator atom (0)
///
/// `tree` in hoon:
///
///
/// ++  tree
///   |$  [node]
///   ::    tree mold generator
///   ::
///   ::  a `++tree` can be empty, or contain a node of a type and
///   ::  left/right sub `++tree` of the same type. pretty-printed with `{}`.
///   ::
///   $@(~ [n=node l=(tree node) r=(tree node)])
///
impl<K: NounDecode, V: NounDecode> NounDecode for HashMap<K, V>
where
    K: std::hash::Hash + Eq + std::fmt::Debug,
    V: std::fmt::Debug,
{
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        trace!("\nDecoding HashMap from noun: {:?}", noun);
        // Handle empty tree case
        if let Ok(atom) = noun.as_atom() {
            trace!("Got atom: {:?}", atom);
            if atom.as_u64()? == 0 {
                return Ok(HashMap::new());
            }
            return Err(NounDecodeError::ExpectedCell);
        }

        let mut map = HashMap::new();

        // Helper function to recursively traverse the tree
        fn traverse_tree<
            K: NounDecode + std::hash::Hash + Eq + std::fmt::Debug,
            V: NounDecode + std::fmt::Debug,
        >(
            node: &Noun,
            map: &mut HashMap<K, V>,
        ) -> Result<(), NounDecodeError> {
            // Base case: empty branch
            if let Ok(atom) = node.as_atom() {
                if atom.as_u64()? == 0 {
                    return Ok(());
                }
                return Err(NounDecodeError::ExpectedCell);
            }

            let cell = node.as_cell()?;

            // Get the key-value pair from the node
            let pair = cell.head().as_cell().map_err(|e| {
                trace!("Failed to get node cell: {:?}", e);
                NounDecodeError::ExpectedCell
            })?;

            trace!(
                "Got node - key: {:?}, value: {:?}",
                pair.head(),
                pair.tail()
            );
            trace!("Key type: {:?}", std::any::type_name::<K>());
            trace!("Value type: {:?}", std::any::type_name::<V>());

            let key = K::from_noun(&pair.head())?;
            let value = V::from_noun(&pair.tail())?;
            trace!("Key: {:?}, Value: {:?}", key, value);
            map.insert(key, value);

            // Get left and right branches
            let rest = cell.tail().as_cell()?;
            let left = &rest.head();
            let right = &rest.tail();

            // Recursively process left and right branches
            traverse_tree(left, map)?;
            traverse_tree(right, map)?;

            Ok(())
        }

        traverse_tree(noun, &mut map)?;
        Ok(map)
    }
}

/// Implements noun encoding for Result types.
///
/// Results are encoded as tagged cells in the following format:
/// ```text
/// Ok(v)  -> [%ok value]    // Cell with 'ok' tag and encoded value
/// Err(e) -> [%err value]   // Cell with 'err' tag and encoded error
/// ```
///
/// This matches Hoon's typical tagged union representation where the head of the cell
/// contains a symbol (term) indicating the variant, and the tail contains the value.
///
/// # Type Parameters
///
/// * `T`: The Ok variant type that implements NounEncode
/// * `E`: The Err variant type that implements NounEncode
///
/// # Examples
///
/// ```rust
/// # use noun_serde::{NounEncode, NounDecode};
/// # use nockvm::mem::NockStack;
/// let result: Result<u64, String> = Ok(42);
///
/// let mut stack = NockStack::new(8 << 10 << 10, 0);
/// let encoded = result.to_noun(&mut stack);
/// let decoded = Result::<u64, String>::from_noun(&encoded).unwrap();
/// assert_eq!(result, decoded);
/// ```
impl<T: NounEncode, E: NounEncode> NounEncode for Result<T, E> {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        match self {
            Ok(v) => {
                let tag = make_tas(allocator, "ok").as_noun();
                let value = v.to_noun(allocator);
                T(allocator, &[tag, value])
            }
            Err(e) => {
                let tag = make_tas(allocator, "err").as_noun();
                let value = e.to_noun(allocator);
                T(allocator, &[tag, value])
            }
        }
    }
}

/// Implements noun decoding for Result types.
///
/// Decodes tagged cells back into Result values, expecting the format:
/// ```text
/// [%ok value]  -> Ok(value)    // 'ok' tag with encoded value
/// [%err value] -> Err(value)   // 'err' tag with encoded error
/// ```
///
/// # Type Parameters
///
/// * `T`: The Ok variant type that implements NounDecode
/// * `E`: The Err variant type that implements NounDecode
///
/// # Errors
///
/// Returns `NounDecodeError` if:
/// - The noun is not a cell
/// - The head is not an atom containing "ok" or "err"
/// - The tail fails to decode as the appropriate type
///
/// # Implementation Notes
///
/// The decoding process:
/// 1. Extracts the tag from the head of the cell
/// 2. Matches on "ok" or "err" to determine the variant
/// 3. Decodes the tail into the appropriate type
/// 4. Wraps in Ok/Err accordingly
impl<T: NounDecode, E: NounDecode> NounDecode for Result<T, E> {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        trace!("\nDecoding Result from noun: {:?}", noun);
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        trace!("Result cell head: {:?}", cell.head());
        trace!("Result cell tail: {:?}", cell.tail());

        let tag = cell
            .head()
            .as_atom()
            .map_err(|_| NounDecodeError::ExpectedAtom)?
            .into_string()
            .map_err(|_| NounDecodeError::InvalidTag)?;

        trace!("Result tag: {}", tag);
        match tag.as_str() {
            "ok" => {
                trace!("Decoding Ok variant");
                Ok(Ok(T::from_noun(&cell.tail())?))
            }
            "err" => {
                trace!("Decoding Err variant");
                Ok(Err(E::from_noun(&cell.tail())?))
            }
            _ => {
                trace!("Invalid Result tag: {}", tag);
                Err(NounDecodeError::InvalidEnumVariant)
            }
        }
    }
}

// Helper function for encoding/decoding bool
pub fn encode_bool(value: bool) -> Noun {
    if value {
        YES
    } else {
        NO
    }
}

pub fn decode_bool(noun: &Noun) -> Result<bool, NounDecodeError> {
    trace!("Decoding bool from noun: {:?}", noun);
    match noun.as_atom() {
        Ok(atom) => {
            trace!("Successfully decoded as atom: {:?}", atom);
            match atom.as_u64() {
                Ok(0) => {
                    trace!("Decoded as 0 -> true (%.y)");
                    Ok(true)
                }
                Ok(1) => {
                    trace!("Decoded as 1 -> false (%.n)");
                    Ok(false)
                }
                other => {
                    trace!("Invalid boolean value: {:?}", other);
                    Err(NounDecodeError::Custom("Invalid boolean value".into()))
                }
            }
        }
        Err(e) => {
            trace!("Failed to decode as atom: {:?}", e);
            Err(NounDecodeError::ExpectedAtom)
        }
    }
}

/// Implements noun encoding for HashSet types.
///
/// HashSet is encoded as a hoon `$set`, which is the same as a `$map` but
/// where the node type is not necessarily a pair.
///
///
/// ```hoon
/// ++  set
///   |$  [item]                                            ::  set
///   $|  (tree item)
///   |=(a=(tree) ?:(=(~ a) & ~(apt in a)))
/// ```
///
/// # Type Parameters
///
/// * `T`: Value type that implements NounEncode + Hash + Eq
///
/// # Examples
///
/// ```rust
/// # use std::collections::HashSet;
/// # use noun_serde::{NounEncode, NounDecode};
/// # use nockvm::mem::NockStack;
/// let mut set = HashSet::new();
/// set.insert("key1".to_string());
/// set.insert("key2".to_string());
///
/// let mut stack = NockStack::new(8 << 10 << 10, 0);
/// let encoded = set.to_noun(&mut stack);
/// let decoded = HashSet::<String>::from_noun(&encoded).unwrap();
/// assert_eq!(set, decoded);
/// ```
impl<T: NounEncode + Clone> NounEncode for HashSet<T>
where
    T: std::hash::Hash + Eq,
{
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        if self.is_empty() {
            return D(0);
        }

        let entries: Vec<_> = self.iter().collect();

        fn build_tree<A: NounAllocator, T: NounEncode>(allocator: &mut A, entries: &[&T]) -> Noun {
            if entries.is_empty() {
                return D(0);
            }

            let mid = entries.len() / 2;
            let node = entries[mid].to_noun(allocator);
            let left = build_tree(allocator, &entries[..mid]);
            let right = build_tree(allocator, &entries[mid + 1..]);

            T(allocator, &[node, left, right])
        }

        build_tree(allocator, &entries)
    }
}

/// Implements noun decoding for HashSet types.
///
/// The decoding process walks through the binary tree structure created by the encoder,
/// extracting values until it hits the terminator atom (0).
///
/// # Type Parameters
///
/// * `T`: Value type that implements NounDecode + Hash + Eq
///
/// # Errors
///
/// Returns `NounDecodeError` if:
/// - The noun structure doesn't match the expected binary tree format
/// - The list isn't properly terminated with atom 0
/// - Any value fails to decode
///
/// # Implementation Notes
///
/// The decoding process:
/// 1. Starts with an empty HashSet
/// 2. For each cell in the chain:
///    - Head contains a value
///    - Tail points to next value or terminator
/// 3. Continues until it hits the terminator atom (0)
impl<T: NounDecode> NounDecode for HashSet<T>
where
    T: std::hash::Hash + Eq,
{
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        // Handle empty tree case
        if let Ok(atom) = noun.as_atom() {
            if atom.as_u64()? == 0 {
                return Ok(HashSet::new());
            }
            return Err(NounDecodeError::ExpectedCell);
        }

        let mut set = HashSet::new();

        // Helper function to recursively traverse the tree
        fn traverse_tree<T: NounDecode + std::hash::Hash + Eq>(
            node: &Noun,
            set: &mut HashSet<T>,
        ) -> Result<(), NounDecodeError> {
            // Base case: empty branch
            if let Ok(atom) = node.as_atom() {
                if atom.as_u64()? == 0 {
                    return Ok(());
                }
                return Err(NounDecodeError::ExpectedCell);
            }

            let cell = node.as_cell()?;

            // Insert the node value
            let value = T::from_noun(&cell.head())?;
            set.insert(value);

            // Get left and right branches
            let rest = cell.tail().as_cell()?;
            let left = &rest.head();
            let right = &rest.tail();

            // Recursively process left and right branches
            traverse_tree(left, set)?;
            traverse_tree(right, set)?;

            Ok(())
        }

        traverse_tree(noun, &mut set)?;
        Ok(set)
    }
}

/// Implements noun encoding for BTreeMap types to match Hoon's map structure.
///
/// BTreeMap is encoded as a Hoon `$map`, which is a binary tree structure:
/// ```text
/// $@(~ [n=[key value] l=(tree [key value]) r=(tree [key value])])
/// ```
///
/// The encoding creates a balanced binary tree where each node contains a [key value] pair
/// and left/right subtrees. Empty trees are represented as atom 0.
///
/// # Type Parameters
///
/// * `K`: Key type that implements NounEncode + Ord
/// * `V`: Value type that implements NounEncode
///
/// # Examples
///
/// ```rust
/// # use std::collections::BTreeMap;
/// # use noun_serde::{NounEncode, NounDecode};
/// # use nockvm::mem::NockStack;
/// let mut map = BTreeMap::new();
/// map.insert(1u64, "value1".to_string());
/// map.insert(2u64, "value2".to_string());
///
/// let mut stack = NockStack::new(8 << 10 << 10, 0);
/// let encoded = map.to_noun(&mut stack);
/// let decoded = BTreeMap::<u64, String>::from_noun(&encoded).unwrap();
/// assert_eq!(map, decoded);
/// ```
impl<K: NounEncode + Ord, V: NounEncode> NounEncode for BTreeMap<K, V> {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        if self.is_empty() {
            return D(0);
        }

        // Convert to sorted vector for balanced tree construction
        let entries: Vec<_> = self.iter().collect();

        fn build_tree<A: NounAllocator, K: NounEncode, V: NounEncode>(
            allocator: &mut A,
            entries: &[(&K, &V)],
        ) -> Noun {
            if entries.is_empty() {
                return D(0);
            }

            // Choose middle element as root for balanced tree
            let mid = entries.len() / 2;
            let (key, value) = entries[mid];

            // Create the node as [key value]
            let key_noun = key.to_noun(allocator);
            let value_noun = value.to_noun(allocator);
            let node = T(allocator, &[key_noun, value_noun]);

            // Recursively build left and right subtrees
            let left = build_tree(allocator, &entries[..mid]);
            let right = build_tree(allocator, &entries[mid + 1..]);

            // Return [node left right]
            T(allocator, &[node, left, right])
        }

        build_tree(allocator, &entries)
    }
}

/// Implements noun decoding for BTreeMap types.
///
/// The decoding process walks through the binary tree structure created by the encoder,
/// extracting key-value pairs from each node and recursively processing left/right subtrees.
///
/// # Type Parameters
///
/// * `K`: Key type that implements NounDecode + Ord
/// * `V`: Value type that implements NounDecode
///
/// # Errors
///
/// Returns `NounDecodeError` if:
/// - The noun structure doesn't match the expected binary tree format
/// - A key-value pair cell is malformed
/// - Any key or value fails to decode
/// - The tree structure is invalid
///
/// # Implementation Notes
///
/// The decoding process:
/// 1. Starts with an empty BTreeMap
/// 2. For each tree node:
///    - Head contains a [key value] cell
///    - Tail contains [left_subtree right_subtree]
/// 3. Recursively processes all subtrees
/// 4. Atom 0 represents empty subtrees
impl<K: NounDecode + Ord, V: NounDecode> NounDecode for BTreeMap<K, V> {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let mut map = BTreeMap::new();

        fn traverse_tree<K: NounDecode + Ord, V: NounDecode>(
            node: &Noun,
            map: &mut BTreeMap<K, V>,
        ) -> Result<(), NounDecodeError> {
            // Base case: empty tree (atom 0)
            if let Ok(atom) = node.as_atom() {
                if atom.as_u64()? == 0 {
                    return Ok(());
                }
                return Err(NounDecodeError::ExpectedCell);
            }

            let cell = node.as_cell()?;

            // Get the [key value] pair from the node
            let pair = cell.head().as_cell()?;
            let key = K::from_noun(&pair.head())?;
            let value = V::from_noun(&pair.tail())?;
            map.insert(key, value);

            // Get left and right subtrees
            let rest = cell.tail().as_cell()?;
            let left = &rest.head();
            let right = &rest.tail();

            // Recursively process left and right subtrees
            traverse_tree(left, map)?;
            traverse_tree(right, map)?;

            Ok(())
        }

        traverse_tree(noun, &mut map)?;
        Ok(map)
    }
}

/// Implements noun encoding for usize.
///
/// usize values are encoded as u64 atoms to ensure compatibility across different
/// architectures and to match Hoon's atom representation.
impl NounEncode for usize {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        (*self as u64).to_noun(allocator)
    }
}

/// Implements noun decoding for usize.
///
/// usize values are decoded from u64 atoms. On 32-bit systems, values larger
/// than u32::MAX will cause a decode error.
impl NounDecode for usize {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let value = u64::from_noun(noun)?;

        // Check if the value fits in usize for the current architecture
        if value > usize::MAX as u64 {
            return Err(NounDecodeError::Custom(format!(
                "Value {} too large for usize on this architecture",
                value
            )));
        }

        Ok(value as usize)
    }
}

impl<T: NounDecode, const N: usize> NounDecode for [T; N] {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let mut result = Vec::with_capacity(N);

        if N > 0 {
            let mut current = *noun;

            // Process all elements except the last one
            for _ in 0..N - 1 {
                let cell = current
                    .as_cell()
                    .map_err(|_| NounDecodeError::ExpectedCell)?;
                let head = cell.head();
                let item = T::from_noun(&head)?;
                result.push(item);
                current = cell.tail();
            }

            // Process the last element
            let last_item = T::from_noun(&current)?;
            result.push(last_item);
        }

        // Convert Vec to array
        result
            .try_into()
            .map_err(|_| NounDecodeError::Custom("Failed to convert Vec to array".into()))
    }
}

impl<T: NounEncode, const N: usize> NounEncode for [T; N] {
    fn to_noun<A: NounAllocator>(&self, stack: &mut A) -> Noun {
        if N == 0 {
            // Empty array is encoded as 0
            return D(0);
        }

        // Start with the last element
        let mut result = self[N - 1].to_noun(stack);

        // Add remaining elements in reverse order
        for i in (0..N - 1).rev() {
            let e = self[i].to_noun(stack);
            result = T(stack, &[e, result]);
        }

        result
    }
}

#[cfg(test)]
mod array_tests {
    use nockvm::jets::util::test::init_context;

    use super::*;

    #[test]
    fn test_empty_array() {
        let mut context = init_context();
        let stack = &mut context.stack;
        let empty_array: [u64; 0] = [];

        // Test encoding
        let noun = empty_array.to_noun(stack);
        assert!(noun.is_atom());
        assert_eq!(noun.as_atom().unwrap().as_u64().unwrap(), 0);

        // Test decoding
        let decoded: [u64; 0] = NounDecode::from_noun(&noun).unwrap();
        assert_eq!(decoded.len(), 0);
    }

    #[test]
    fn test_single_element_array() {
        let mut context = init_context();
        let stack = &mut context.stack;
        let array = [42u64];

        // Test encoding
        let noun = array.to_noun(stack);

        // Encoding should produce just the single element as an atom
        assert!(noun.is_atom());
        assert_eq!(noun.as_atom().unwrap().as_u64().unwrap(), 42);

        // Test decoding
        let decoded: [u64; 1] = NounDecode::from_noun(&noun).unwrap();
        assert_eq!(decoded, [42]);
    }

    #[test]
    fn test_multi_element_array() {
        let mut context = init_context();
        let stack = &mut context.stack;
        let array = [1u64, 2, 3, 4, 5];

        // Test encoding
        let noun = array.to_noun(stack);

        // It should create a right-associative chain of cells
        assert!(noun.is_cell());
        let cell = noun.as_cell().unwrap();
        assert_eq!(cell.head().as_atom().unwrap().as_u64().unwrap(), 1);

        // Recursively check the structure
        let tail1 = cell.tail();
        assert!(tail1.is_cell());
        let cell1 = tail1.as_cell().unwrap();
        assert_eq!(cell1.head().as_atom().unwrap().as_u64().unwrap(), 2);

        let tail2 = cell1.tail();
        assert!(tail2.is_cell());
        let cell2 = tail2.as_cell().unwrap();
        assert_eq!(cell2.head().as_atom().unwrap().as_u64().unwrap(), 3);

        let tail3 = cell2.tail();
        assert!(tail3.is_cell());
        let cell3 = tail3.as_cell().unwrap();
        assert_eq!(cell3.head().as_atom().unwrap().as_u64().unwrap(), 4);

        // Last element should be an atom, not a cell
        let tail4 = cell3.tail();
        assert!(tail4.is_atom());
        assert_eq!(tail4.as_atom().unwrap().as_u64().unwrap(), 5);

        // Test decoding
        let decoded: [u64; 5] = NounDecode::from_noun(&noun).unwrap();
        assert_eq!(decoded, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_string_array() {
        let mut context = init_context();
        let stack = &mut context.stack;
        let array = ["hello".to_string(), "world".to_string()];

        // Test encoding
        let noun = array.to_noun(stack);

        // Test decoding
        let decoded: [String; 2] = NounDecode::from_noun(&noun).unwrap();
        assert_eq!(decoded, ["hello".to_string(), "world".to_string()]);
    }
    #[test]
    fn test_bool_array() {
        let mut context = init_context();
        let stack = &mut context.stack;
        let array = [true, false, true];

        // Test encoding
        let noun = array.to_noun(stack);

        // Test decoding
        let decoded: [bool; 3] = NounDecode::from_noun(&noun).unwrap();
        assert_eq!(decoded, [true, false, true]);
    }
    #[test]
    fn test_nested_array() {
        let mut context = init_context();
        let stack = &mut context.stack;
        let array = [[1u64, 2u64], [3u64, 4u64]];

        // Test encoding
        let noun = array.to_noun(stack);

        // Test decoding
        let decoded: [[u64; 2]; 2] = NounDecode::from_noun(&noun).unwrap();
        assert_eq!(decoded, [[1, 2], [3, 4]]);
    }

    #[test]
    fn test_decode_error() {
        let mut context = init_context();
        let stack = &mut context.stack;

        // Create an atom (not a proper cell chain for array)
        let noun = D(42);

        // Try to decode as a multi-element array (should fail)
        let result = <[u64; 3]>::from_noun(&noun);
        assert!(result.is_err());

        // Create a cell with wrong structure
        let bad_noun = T(stack, &[D(1), D(2)]);

        // Last element should be a cell for a 3-element array
        let result = <[u64; 3]>::from_noun(&bad_noun);
        assert!(result.is_err());
    }
}

#[cfg(test)]
mod tip5_tests {
    use super::*;

    #[test]
    fn test_tip5_encode_decode_array() {
        let mut stack = nockvm::mem::NockStack::new(1024 * 1024, 0);
        let noun = T(&mut stack, &[D(1), D(2), D(3), D(4), D(5)]);
        let decoded = noun.decode::<[u64; 5]>().unwrap();
        assert_eq!([1, 2, 3, 4, 5], decoded);
    }

    #[test]
    fn test_tip5_encode_decode_tuple() {
        let mut stack = nockvm::mem::NockStack::new(1024 * 1024, 0);
        let noun = T(&mut stack, &[D(1), D(2), D(3), D(4), D(5)]);
        let decoded = noun.decode::<(u64, u64, u64, u64, u64)>().unwrap();
        assert_eq!((1, 2, 3, 4, 5), decoded);
    }
}

#[cfg(test)]
mod btreemap_tests {
    use std::collections::BTreeMap;

    use nockvm::mem::NockStack;

    use super::*;

    #[test]
    fn test_btreemap_empty() {
        let mut stack = NockStack::new(1024 * 1024, 0);
        let map: BTreeMap<u64, String> = BTreeMap::new();

        let noun = map.to_noun(&mut stack);

        // Empty map should encode as atom 0
        assert!(noun.as_atom().is_ok());
        assert_eq!(noun.as_atom().unwrap().as_u64().unwrap(), 0);

        // Round-trip test
        let decoded = BTreeMap::<u64, String>::from_noun(&noun).unwrap();
        assert_eq!(map, decoded);
    }

    #[test]
    fn test_btreemap_single_entry() {
        let mut stack = NockStack::new(1024 * 1024, 0);
        let mut map = BTreeMap::new();
        map.insert(42u64, "hello".to_string());

        let noun = map.to_noun(&mut stack);

        // Should be a cell structure [node left right]
        assert!(noun.as_cell().is_ok());

        // Round-trip test
        let decoded = BTreeMap::<u64, String>::from_noun(&noun).unwrap();
        assert_eq!(map, decoded);
        assert_eq!(decoded.get(&42), Some(&"hello".to_string()));
    }

    #[test]
    fn test_btreemap_multiple_entries() {
        let mut stack = NockStack::new(1024 * 1024, 0);
        let mut map = BTreeMap::new();
        map.insert(1u64, "one".to_string());
        map.insert(2u64, "two".to_string());
        map.insert(3u64, "three".to_string());
        map.insert(10u64, "ten".to_string());
        map.insert(5u64, "five".to_string());

        let noun = map.to_noun(&mut stack);

        // Round-trip test
        let decoded = BTreeMap::<u64, String>::from_noun(&noun).unwrap();
        assert_eq!(map, decoded);

        // Check all values are preserved
        assert_eq!(decoded.get(&1), Some(&"one".to_string()));
        assert_eq!(decoded.get(&2), Some(&"two".to_string()));
        assert_eq!(decoded.get(&3), Some(&"three".to_string()));
        assert_eq!(decoded.get(&5), Some(&"five".to_string()));
        assert_eq!(decoded.get(&10), Some(&"ten".to_string()));
        assert_eq!(decoded.len(), 5);
    }

    #[test]
    fn test_btreemap_u64_u64() {
        let mut stack = NockStack::new(1024 * 1024, 0);
        let mut map = BTreeMap::new();
        map.insert(1u64, 100u64);
        map.insert(2u64, 200u64);
        map.insert(3u64, 300u64);

        let noun = map.to_noun(&mut stack);

        // Round-trip test
        let decoded = BTreeMap::<u64, u64>::from_noun(&noun).unwrap();
        assert_eq!(map, decoded);

        // Check specific values
        assert_eq!(decoded.get(&1), Some(&100u64));
        assert_eq!(decoded.get(&2), Some(&200u64));
        assert_eq!(decoded.get(&3), Some(&300u64));
    }

    #[test]
    fn test_btreemap_nested_structure() {
        let mut stack = NockStack::new(1024 * 1024, 0);

        // Create a map of maps
        let mut inner1 = BTreeMap::new();
        inner1.insert("a".to_string(), 1u64);
        inner1.insert("b".to_string(), 2u64);

        let mut inner2 = BTreeMap::new();
        inner2.insert("x".to_string(), 10u64);
        inner2.insert("y".to_string(), 20u64);

        let mut outer = BTreeMap::new();
        outer.insert(1u64, inner1.clone());
        outer.insert(2u64, inner2.clone());

        let noun = outer.to_noun(&mut stack);

        // Round-trip test
        let decoded = BTreeMap::<u64, BTreeMap<String, u64>>::from_noun(&noun).unwrap();
        assert_eq!(outer, decoded);

        // Check nested values
        assert_eq!(decoded.get(&1).unwrap().get("a"), Some(&1u64));
        assert_eq!(decoded.get(&1).unwrap().get("b"), Some(&2u64));
        assert_eq!(decoded.get(&2).unwrap().get("x"), Some(&10u64));
        assert_eq!(decoded.get(&2).unwrap().get("y"), Some(&20u64));
    }

    #[test]
    fn test_btreemap_invalid_noun_structure() {
        let mut stack = NockStack::new(1024 * 1024, 0);

        // Test with invalid atom (not 0)
        let invalid_atom = nockvm::noun::Atom::new(&mut stack, 42).as_noun();
        let result = BTreeMap::<u64, String>::from_noun(&invalid_atom);
        assert!(result.is_err());

        // Test with malformed cell (missing key-value structure)
        let malformed = nockvm::noun::T(
            &mut stack,
            &[
                nockvm::noun::D(1), // Should be [key value] pair
                nockvm::noun::D(0),
                nockvm::noun::D(0),
            ],
        );
        let result = BTreeMap::<u64, String>::from_noun(&malformed);
        assert!(result.is_err());
    }

    #[test]
    fn test_btreemap_ordering_preserved() {
        let mut stack = NockStack::new(1024 * 1024, 0);

        // Insert in random order
        let mut map = BTreeMap::new();
        let values = vec![5u64, 2, 8, 1, 9, 3, 7, 4, 6];
        for (i, &val) in values.iter().enumerate() {
            map.insert(val, format!("value_{}", i));
        }

        let noun = map.to_noun(&mut stack);
        let decoded = BTreeMap::<u64, String>::from_noun(&noun).unwrap();

        // BTreeMap should maintain sorted order
        let original_keys: Vec<_> = map.keys().collect();
        let decoded_keys: Vec<_> = decoded.keys().collect();
        assert_eq!(original_keys, decoded_keys);

        // Keys should be in sorted order
        let mut sorted_keys = values.clone();
        sorted_keys.sort();
        let actual_keys: Vec<_> = decoded.keys().cloned().collect();
        assert_eq!(sorted_keys, actual_keys);
    }

    #[test]
    fn test_usize_encoding() {
        let mut stack = NockStack::new(1024 * 1024, 0);

        // Test various usize values
        let values = vec![0usize, 1, 42, 1000, usize::MAX];

        for &value in &values {
            let noun = value.to_noun(&mut stack);
            let decoded = usize::from_noun(&noun).unwrap();
            assert_eq!(value, decoded);
        }
    }

    #[test]
    fn test_btreemap_usize_belt() {
        let mut stack = NockStack::new(1024 * 1024, 0);
        let mut map = BTreeMap::new();
        map.insert(0usize, 100u64);
        map.insert(1usize, 200u64);
        map.insert(2usize, 300u64);

        let noun = map.to_noun(&mut stack);

        // Round-trip test
        let decoded = BTreeMap::<usize, u64>::from_noun(&noun).unwrap();
        assert_eq!(map, decoded);

        // Check specific values
        assert_eq!(decoded.get(&0), Some(&100u64));
        assert_eq!(decoded.get(&1), Some(&200u64));
        assert_eq!(decoded.get(&2), Some(&300u64));
    }
}
impl NounEncode for Bytes {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        <Atom as AtomExt>::from_bytes(allocator, self.as_ref()).as_noun()
    }
}

impl NounDecode for Bytes {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let atom = noun.as_atom().map_err(|_| NounDecodeError::ExpectedAtom)?;
        let bytes = atom.as_ne_bytes().to_vec();
        Ok(Bytes::from(bytes))
    }
}
