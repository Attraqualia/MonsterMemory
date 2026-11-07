//! Order-preserving index-key encoding.

use smallvec::SmallVec;
use std::fmt;

/// One encoded component of an exact or composite index key.
///
/// Equality, hashing, and ordering compare the encoded bytes. Components are kept
/// separate inside [`IndexKey`], so variable-length values do not need separators.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct KeyPart(SmallVec<[u8; 32]>);

impl fmt::Debug for KeyPart {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("KeyPart")
            .field(&format_args!("{} bytes", self.0.len()))
            .finish()
    }
}

impl KeyPart {
    /// Encodes a value stored in an index.
    ///
    /// Uses [`IndexValue::encode_index`]; values with the same encoding compare
    /// equal even if their original Rust values differ.
    #[must_use]
    pub fn from_value<T: IndexValue + ?Sized>(value: &T) -> Self {
        let mut bytes = SmallVec::new();
        value.encode_index(&mut bytes);
        Self(bytes)
    }

    /// Encodes a borrowed lookup value accepted for `Stored`.
    ///
    /// Uses [`Lookup::encode_lookup`]. For example, a `str` lookup addresses a
    /// stored `String` without allocating an intermediate `String`.
    ///
    /// # Examples
    ///
    /// ```
    /// use monstermemory_core::KeyPart;
    /// assert_eq!(
    ///     KeyPart::from_lookup::<String, str>("JP"),
    ///     KeyPart::from_value(&String::from("JP")),
    /// );
    /// ```
    #[must_use]
    pub fn from_lookup<Stored: ?Sized, Q: Lookup<Stored> + ?Sized>(value: &Q) -> Self {
        let mut bytes = SmallVec::new();
        value.encode_lookup(&mut bytes);
        Self(bytes)
    }
}

/// A lexicographically ordered, possibly composite index key.
///
/// Components compare in their supplied order. If every shared component is
/// equal, a shorter key sorts before a longer one. Key encodings are an in-memory
/// comparison representation, not a stable persistence or interchange format.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct IndexKey(SmallVec<[KeyPart; 3]>);

impl IndexKey {
    /// Creates a single-field key.
    #[must_use]
    pub fn single(part: KeyPart) -> Self {
        let mut parts = SmallVec::new();
        parts.push(part);
        Self(parts)
    }

    /// Creates a composite key from a fixed-size field array.
    ///
    /// Array order is comparison order; a zero-length array creates the empty key.
    /// See the [crate example](crate) for ordered composite keys.
    #[must_use]
    pub fn from_parts<const N: usize>(parts: [KeyPart; N]) -> Self {
        Self(parts.into_iter().collect())
    }

    /// Returns true when all components in `prefix` match this key.
    ///
    /// Matching uses complete encoded components. A string component `"J"` does
    /// not match `"JP"`; an empty key is a prefix of every key.
    #[must_use]
    pub fn starts_with(&self, prefix: &Self) -> bool {
        self.0.as_slice().starts_with(prefix.0.as_slice())
    }

    /// Appends a component and returns the resulting key.
    #[must_use]
    pub fn with_part(mut self, part: KeyPart) -> Self {
        self.0.push(part);
        self
    }
}

/// Converts an indexable value to bytes whose lexical order matches its value order.
///
/// Floating-point types are intentionally omitted because their partial ordering is
/// unsuitable for an ordered database index without an explicit total-order policy.
///
/// Implementations must produce deterministic encodings: equal logical values
/// need equal bytes, and values that must be distinct in an index need different
/// bytes. Lexicographic byte order must match the intended index ordering. Keep
/// this encoding consistent with every corresponding [`Lookup`] implementation.
///
/// Built-in implementations cover integers, `bool`, `char`, UTF-8 strings, byte
/// slices/vectors, and `Option<T>`. Strings compare by UTF-8 bytes without locale
/// rules or normalization. `None` sorts before `Some`, including in unique indexes
/// where duplicate `None` values are rejected.
pub trait IndexValue {
    /// Appends the canonical encoding to `output`.
    ///
    /// Preserve bytes already in the buffer. The engine uses a fresh buffer for
    /// each component, so lengths or terminators are not needed between components.
    fn encode_index(&self, output: &mut SmallVec<[u8; 32]>);
}

/// Allows a borrowed query value to address a stored field type.
///
/// Matching query and stored values must encode identically, with the ordering
/// required by [`IndexValue`]. A blanket implementation accepts the stored type
/// itself; `str` also looks up `String`, and `[u8]` looks up `Vec<u8>`.
pub trait Lookup<Stored: ?Sized> {
    /// Appends the canonical lookup encoding to `output`.
    ///
    /// Preserve existing bytes and use the same bytes as the matching `Stored`
    /// value's [`IndexValue::encode_index`] implementation.
    fn encode_lookup(&self, output: &mut SmallVec<[u8; 32]>);
}

impl<T: IndexValue + ?Sized> Lookup<T> for T {
    fn encode_lookup(&self, output: &mut SmallVec<[u8; 32]>) {
        self.encode_index(output);
    }
}

impl Lookup<String> for str {
    fn encode_lookup(&self, output: &mut SmallVec<[u8; 32]>) {
        self.encode_index(output);
    }
}

impl Lookup<Vec<u8>> for [u8] {
    fn encode_lookup(&self, output: &mut SmallVec<[u8; 32]>) {
        self.encode_index(output);
    }
}

macro_rules! impl_unsigned {
    ($($ty:ty),* $(,)?) => {$ (
        impl IndexValue for $ty {
            fn encode_index(&self, output: &mut SmallVec<[u8; 32]>) {
                output.extend_from_slice(&self.to_be_bytes());
            }
        }
    )* };
}

macro_rules! impl_signed {
    ($(($ty:ty, $unsigned:ty)),* $(,)?) => {$ (
        impl IndexValue for $ty {
            fn encode_index(&self, output: &mut SmallVec<[u8; 32]>) {
                // 符号ビットを反転し、負数を正数より前に並べるバイト順へ写像する。
                let shifted = self.cast_unsigned() ^ (1 << (<$ty>::BITS - 1));
                output.extend_from_slice(&shifted.to_be_bytes());
            }
        }
    )* };
}

impl_unsigned!(u8, u16, u32, u64, u128);
impl_signed!((i8, u8), (i16, u16), (i32, u32), (i64, u64), (i128, u128));

impl IndexValue for usize {
    fn encode_index(&self, output: &mut SmallVec<[u8; 32]>) {
        output.extend_from_slice(&(*self as u64).to_be_bytes());
    }
}

impl IndexValue for isize {
    fn encode_index(&self, output: &mut SmallVec<[u8; 32]>) {
        (*self as i64).encode_index(output);
    }
}

impl IndexValue for bool {
    fn encode_index(&self, output: &mut SmallVec<[u8; 32]>) {
        output.push(u8::from(*self));
    }
}

impl IndexValue for char {
    fn encode_index(&self, output: &mut SmallVec<[u8; 32]>) {
        (*self as u32).encode_index(output);
    }
}

impl IndexValue for str {
    fn encode_index(&self, output: &mut SmallVec<[u8; 32]>) {
        output.extend_from_slice(self.as_bytes());
    }
}

impl IndexValue for String {
    fn encode_index(&self, output: &mut SmallVec<[u8; 32]>) {
        self.as_str().encode_index(output);
    }
}

impl IndexValue for [u8] {
    fn encode_index(&self, output: &mut SmallVec<[u8; 32]>) {
        output.extend_from_slice(self);
    }
}

impl IndexValue for Vec<u8> {
    fn encode_index(&self, output: &mut SmallVec<[u8; 32]>) {
        self.as_slice().encode_index(output);
    }
}

impl<T: IndexValue> IndexValue for Option<T> {
    fn encode_index(&self, output: &mut SmallVec<[u8; 32]>) {
        match self {
            None => output.push(0),
            Some(value) => {
                output.push(1);
                value.encode_index(output);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{IndexValue as _, KeyPart};
    use smallvec::SmallVec;

    #[test]
    fn signed_encoding_preserves_order() {
        let values = [-100_i32, -1, 0, 1, 100];
        let encoded: Vec<_> = values.iter().map(KeyPart::from_value).collect();
        assert!(encoded.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn strings_use_their_lexical_order() {
        let mut bytes = SmallVec::<[u8; 32]>::new();
        "monster".encode_index(&mut bytes);
        assert_eq!(bytes.as_slice(), b"monster");
    }
}
