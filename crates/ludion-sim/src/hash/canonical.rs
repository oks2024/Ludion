//! The canonical encoder behind [`WorldHash`](super::WorldHash): a `serde::Serializer` that writes
//! rule 5 of the parent module's normative byte rules straight into a reusable buffer.
//!
//! # Why a serializer
//!
//! The hash visits every registered component of every live entity, every frame. Routing that
//! through `serde_json::to_value` builds — and immediately drops — a heap tree per component, which
//! measured as the dominant cost of hashing (a 6.5k-entity world spent ~32 ms per hash building and
//! walking JSON trees, against ~1 ms for the entity/component walk itself). Encoding straight from
//! the component's own `Serialize` skips the tree entirely.
//!
//! # Compatibility contract
//!
//! The byte stream is frozen (`ludion.worldhash.v2`) and is identical to canonically encoding
//! `serde_json::to_value(component)`, except for non-finite floats and `None`, which `Value` cannot
//! tell apart from each other or from unit. Every method below mirrors what `serde_json`'s own
//! `Value` serializer would have *built*, not what JSON text would look like:
//!
//! - `i8`/`i16`/`i32` collapse to `i64` and `u8`/`u16`/`u32` to `u64`, exactly as `Value` does;
//!   `i128`/`u128` narrow to `u64` (preferred) or `i64` and are otherwise out of range.
//! - **`f32` widens to `f64` and is then formatted as an `f64`** — `Value::from(f32)` stores
//!   `f as f64` in a `Number`, whose `Display` is the only number-formatting path used here, so the
//!   decimal text can never drift from `serde_json`'s.
//! - A non-finite float is the number tag over the text `NaN`, `inf` or `-inf` — a finite number's
//!   text never contains those letters, so they cannot collide. A NaN's sign and payload are not
//!   hashed (they differ between x86 and ARM).
//! - `None` is its own one-byte tag, distinct from unit.
//! - `char` becomes a one-character string; `bytes` become an array of `u8` numbers; `unit` and
//!   `unit_struct` become `null`; the variant forms become the single-key objects `Value` builds
//!   for them.
//! - An object's entries are deduplicated keeping the **last** value written for a key, because that
//!   is what `serde_json::Map::insert` does, and then sorted by key bytes per rule 5.
//!
//! `hash::tests` pins all of this against the pre-optimization `Value` walker, which is kept
//! verbatim as the oracle.

use std::fmt::Display;
use std::io::Write as _;

use serde::ser::{Error as _, Impossible, Serialize, Serializer};
use serde_json::{Error, Number};

/// One object entry already written into the buffer, as `[start, end)`: a `u64` key-length prefix,
/// `key_len` key bytes, then the value's encoding. Recorded rather than emitted so the whole set can
/// be sorted (rule 5) once the object closes.
#[derive(Clone, Copy)]
struct Entry {
    start: usize,
    key_len: usize,
    end: usize,
}

fn key_bytes<'a>(out: &'a [u8], entry: &Entry) -> &'a [u8] {
    &out[entry.start + 8..entry.start + 8 + entry.key_len]
}

/// An open container. Both kinds patch a `u64` count back into the buffer on close, since serde may
/// not know a length up front and an object's entry count is only final after deduplication.
enum Frame {
    Seq {
        count_at: usize,
        count: u64,
    },
    Object {
        count_at: usize,
        entries: Vec<Entry>,
        /// `(start, key_len)` of the entry whose key is written but whose value is not — the state
        /// `SerializeMap` sits in between `serialize_key` and `serialize_value`.
        pending: Option<(usize, usize)>,
    },
}

/// The canonical encoder. One instance serves a whole world walk: `out` and the two pools grow to
/// the largest component encountered and are then reused, so steady-state encoding allocates
/// nothing.
#[derive(Default)]
pub(super) struct Canonical {
    out: Vec<u8>,
    frames: Vec<Frame>,
    entry_pool: Vec<Vec<Entry>>,
    byte_pool: Vec<Vec<u8>>,
    non_finite: u64,
}

impl Canonical {
    /// How many non-finite floats the last `encode` wrote.
    pub(super) fn non_finite(&self) -> u64 {
        self.non_finite
    }

    /// Encodes one component, returning its canonical bytes (valid until the next `encode`).
    ///
    /// # Errors
    /// Whatever the component's own `Serialize` reports, plus the two shapes JSON cannot hold: a
    /// 128-bit integer outside `u64`/`i64`, and a map key that is not string-like.
    pub(super) fn encode(
        &mut self,
        component: &dyn erased_serde::Serialize,
    ) -> Result<&[u8], Error> {
        self.out.clear();
        self.frames.clear();
        self.non_finite = 0;
        erased_serde::serialize(component, &mut *self)?;
        debug_assert!(self.frames.is_empty(), "every container was closed");
        Ok(&self.out)
    }

    fn push_len_prefixed(&mut self, bytes: &[u8]) {
        self.out
            .extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        self.out.extend_from_slice(bytes);
    }

    /// Writes `tag` followed by the length-prefixed `Display` rendering of `value`. The length is
    /// only known after formatting, so it is reserved and patched.
    fn push_displayed(&mut self, tag: u8, value: &dyn Display) {
        self.out.push(tag);
        let at = self.out.len();
        self.out.extend_from_slice(&0u64.to_le_bytes());
        write!(self.out, "{value}").expect("formatting into a Vec cannot fail");
        let len = (self.out.len() - at - 8) as u64;
        self.out[at..at + 8].copy_from_slice(&len.to_le_bytes());
    }

    /// Formats through `serde_json::Number`'s own `Display` — the single source of the decimal text
    /// (itoa for integers, shortest round-trip for floats), so it cannot drift from what a `Value`
    /// carrying the same number would have produced.
    fn push_number(&mut self, number: &Number) {
        self.push_displayed(b'#', number);
    }

    fn push_str(&mut self, value: &str) {
        self.out.push(b's');
        self.push_len_prefixed(value.as_bytes());
    }

    fn begin_seq(&mut self) {
        self.out.push(b'[');
        let count_at = self.out.len();
        self.out.extend_from_slice(&0u64.to_le_bytes());
        self.frames.push(Frame::Seq { count_at, count: 0 });
    }

    /// Counts the element that just finished writing.
    fn count_element(&mut self) {
        match self.frames.last_mut() {
            Some(Frame::Seq { count, .. }) => *count += 1,
            _ => unreachable!("an element only closes inside the sequence that opened it"),
        }
    }

    fn end_seq(&mut self) {
        let Some(Frame::Seq { count_at, count }) = self.frames.pop() else {
            unreachable!("serde closes the sequence frame it opened")
        };
        self.out[count_at..count_at + 8].copy_from_slice(&count.to_le_bytes());
    }

    fn begin_object(&mut self) {
        self.out.push(b'{');
        let count_at = self.out.len();
        self.out.extend_from_slice(&0u64.to_le_bytes());
        let entries = self.entry_pool.pop().unwrap_or_default();
        self.frames.push(Frame::Object {
            count_at,
            entries,
            pending: None,
        });
    }

    fn push_key(&mut self, key: &str) {
        let start = self.out.len();
        self.push_len_prefixed(key.as_bytes());
        match self.frames.last_mut() {
            Some(Frame::Object { pending, .. }) => *pending = Some((start, key.len())),
            _ => unreachable!("a key is only written inside the object that opened it"),
        }
    }

    /// Closes the entry whose value just finished writing.
    fn close_entry(&mut self) {
        let end = self.out.len();
        match self.frames.last_mut() {
            Some(Frame::Object {
                entries, pending, ..
            }) => {
                let (start, key_len) = pending.take().expect("a value always follows its key");
                entries.push(Entry {
                    start,
                    key_len,
                    end,
                });
            }
            _ => unreachable!("an entry only closes inside the object that opened it"),
        }
    }

    fn end_object(&mut self) {
        let Some(Frame::Object {
            count_at,
            mut entries,
            ..
        }) = self.frames.pop()
        else {
            unreachable!("serde closes the object frame it opened")
        };
        {
            let out = &self.out;
            // Key ascending, position descending: the survivor of a repeated key sorts first in its
            // run, and `dedup_by` keeps the first — which is the *last* value written, matching
            // `serde_json::Map::insert`. After the dedup the order is rule 5's sort by key bytes.
            entries.sort_by(|a, b| {
                key_bytes(out, a)
                    .cmp(key_bytes(out, b))
                    .then(b.start.cmp(&a.start))
            });
            entries.dedup_by(|a, b| key_bytes(out, a) == key_bytes(out, b));
        }
        let body = count_at + 8;
        let mut sorted = self.byte_pool.pop().unwrap_or_default();
        sorted.clear();
        for entry in &entries {
            sorted.extend_from_slice(&self.out[entry.start..entry.end]);
        }
        self.out.truncate(body);
        self.out.extend_from_slice(&sorted);
        self.out[count_at..body].copy_from_slice(&(entries.len() as u64).to_le_bytes());
        sorted.clear();
        self.byte_pool.push(sorted);
        entries.clear();
        self.entry_pool.push(entries);
    }
}

fn key_must_be_a_string() -> Error {
    Error::custom("key must be a string")
}

fn float_key_must_be_finite() -> Error {
    Error::custom("float key must be finite")
}

fn number_out_of_range() -> Error {
    Error::custom("number out of range")
}

impl Serializer for &mut Canonical {
    type Ok = ();
    type Error = Error;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    fn serialize_bool(self, value: bool) -> Result<(), Error> {
        self.out.push(if value { b't' } else { b'f' });
        Ok(())
    }

    fn serialize_i8(self, value: i8) -> Result<(), Error> {
        self.serialize_i64(value as i64)
    }

    fn serialize_i16(self, value: i16) -> Result<(), Error> {
        self.serialize_i64(value as i64)
    }

    fn serialize_i32(self, value: i32) -> Result<(), Error> {
        self.serialize_i64(value as i64)
    }

    fn serialize_i64(self, value: i64) -> Result<(), Error> {
        self.push_number(&Number::from(value));
        Ok(())
    }

    fn serialize_i128(self, value: i128) -> Result<(), Error> {
        if let Ok(value) = u64::try_from(value) {
            self.serialize_u64(value)
        } else if let Ok(value) = i64::try_from(value) {
            self.serialize_i64(value)
        } else {
            Err(number_out_of_range())
        }
    }

    fn serialize_u8(self, value: u8) -> Result<(), Error> {
        self.serialize_u64(value as u64)
    }

    fn serialize_u16(self, value: u16) -> Result<(), Error> {
        self.serialize_u64(value as u64)
    }

    fn serialize_u32(self, value: u32) -> Result<(), Error> {
        self.serialize_u64(value as u64)
    }

    fn serialize_u64(self, value: u64) -> Result<(), Error> {
        self.push_number(&Number::from(value));
        Ok(())
    }

    fn serialize_u128(self, value: u128) -> Result<(), Error> {
        match u64::try_from(value) {
            Ok(value) => self.serialize_u64(value),
            Err(_) => Err(number_out_of_range()),
        }
    }

    /// Widens to `f64` first — `Value::from(f32)` stores `f as f64`, so the `f32` never reaches a
    /// formatter as an `f32` and the two paths cannot disagree.
    fn serialize_f32(self, value: f32) -> Result<(), Error> {
        self.serialize_f64(value as f64)
    }

    fn serialize_f64(self, value: f64) -> Result<(), Error> {
        match Number::from_f64(value) {
            Some(number) => self.push_number(&number),
            // Sign and payload of a NaN are not hashed: x86 and ARM disagree on the default NaN
            // bits, so hashing them would break cross-arch identity.
            None => {
                self.non_finite += 1;
                let text = if value.is_nan() {
                    "NaN"
                } else if value.is_sign_positive() {
                    "inf"
                } else {
                    "-inf"
                };
                self.push_displayed(b'#', &text);
            }
        }
        Ok(())
    }

    fn serialize_char(self, value: char) -> Result<(), Error> {
        let mut buffer = [0u8; 4];
        self.push_str(value.encode_utf8(&mut buffer));
        Ok(())
    }

    fn serialize_str(self, value: &str) -> Result<(), Error> {
        self.push_str(value);
        Ok(())
    }

    /// An array of `u8` numbers — the `Value` a byte string builds, not a JSON string.
    fn serialize_bytes(self, value: &[u8]) -> Result<(), Error> {
        self.begin_seq();
        for &byte in value {
            self.push_number(&Number::from(byte));
            self.count_element();
        }
        self.end_seq();
        Ok(())
    }

    fn serialize_none(self) -> Result<(), Error> {
        self.out.push(b'o');
        Ok(())
    }

    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<(), Error> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<(), Error> {
        self.out.push(b'n');
        Ok(())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<(), Error> {
        self.serialize_unit()
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<(), Error> {
        self.serialize_str(variant)
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        self.begin_object();
        self.push_key(variant);
        value.serialize(&mut *self)?;
        self.close_entry();
        self.end_object();
        Ok(())
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Self, Error> {
        self.begin_seq();
        Ok(self)
    }

    fn serialize_tuple(self, len: usize) -> Result<Self, Error> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_struct(self, _name: &'static str, len: usize) -> Result<Self, Error> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Self, Error> {
        self.begin_object();
        self.push_key(variant);
        self.begin_seq();
        Ok(self)
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self, Error> {
        self.begin_object();
        Ok(self)
    }

    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<Self, Error> {
        self.begin_object();
        Ok(self)
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Self, Error> {
        self.begin_object();
        self.push_key(variant);
        self.begin_object();
        Ok(self)
    }

    fn collect_str<T: ?Sized + Display>(self, value: &T) -> Result<(), Error> {
        // `Value::String(value.to_string())` — the same `Display` rendering, without the `String`.
        self.push_displayed(b's', &value);
        Ok(())
    }
}

impl serde::ser::SerializeSeq for &mut Canonical {
    type Ok = ();
    type Error = Error;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
        value.serialize(&mut **self)?;
        self.count_element();
        Ok(())
    }

    fn end(self) -> Result<(), Error> {
        self.end_seq();
        Ok(())
    }
}

impl serde::ser::SerializeTuple for &mut Canonical {
    type Ok = ();
    type Error = Error;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
        serde::ser::SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> Result<(), Error> {
        self.end_seq();
        Ok(())
    }
}

impl serde::ser::SerializeTupleStruct for &mut Canonical {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
        serde::ser::SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> Result<(), Error> {
        self.end_seq();
        Ok(())
    }
}

impl serde::ser::SerializeTupleVariant for &mut Canonical {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
        serde::ser::SerializeSeq::serialize_element(self, value)
    }

    /// Closes the array *and* the single-entry object wrapping it under the variant name.
    fn end(self) -> Result<(), Error> {
        self.end_seq();
        self.close_entry();
        self.end_object();
        Ok(())
    }
}

impl serde::ser::SerializeMap for &mut Canonical {
    type Ok = ();
    type Error = Error;

    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), Error> {
        key.serialize(MapKey { encoder: self })
    }

    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
        value.serialize(&mut **self)?;
        self.close_entry();
        Ok(())
    }

    fn end(self) -> Result<(), Error> {
        self.end_object();
        Ok(())
    }
}

impl serde::ser::SerializeStruct for &mut Canonical {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        self.push_key(key);
        value.serialize(&mut **self)?;
        self.close_entry();
        Ok(())
    }

    fn end(self) -> Result<(), Error> {
        self.end_object();
        Ok(())
    }
}

impl serde::ser::SerializeStructVariant for &mut Canonical {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        serde::ser::SerializeStruct::serialize_field(self, key, value)
    }

    /// Closes the inner object *and* the single-entry object wrapping it under the variant name.
    fn end(self) -> Result<(), Error> {
        self.end_object();
        self.close_entry();
        self.end_object();
        Ok(())
    }
}

/// Serializes a map key, writing it length-prefixed into the encoder's buffer.
///
/// Mirrors `serde_json`'s own map-key serializer: string-like scalars render to the text they would
/// have taken as a JSON object key, and everything else is rejected — a JSON object cannot hold it.
struct MapKey<'a> {
    encoder: &'a mut Canonical,
}

impl Serializer for MapKey<'_> {
    type Ok = ();
    type Error = Error;
    type SerializeSeq = Impossible<(), Error>;
    type SerializeTuple = Impossible<(), Error>;
    type SerializeTupleStruct = Impossible<(), Error>;
    type SerializeTupleVariant = Impossible<(), Error>;
    type SerializeMap = Impossible<(), Error>;
    type SerializeStruct = Impossible<(), Error>;
    type SerializeStructVariant = Impossible<(), Error>;

    fn serialize_bool(self, value: bool) -> Result<(), Error> {
        self.encoder.push_key(if value { "true" } else { "false" });
        Ok(())
    }

    fn serialize_i8(self, value: i8) -> Result<(), Error> {
        self.serialize_i128(value as i128)
    }

    fn serialize_i16(self, value: i16) -> Result<(), Error> {
        self.serialize_i128(value as i128)
    }

    fn serialize_i32(self, value: i32) -> Result<(), Error> {
        self.serialize_i128(value as i128)
    }

    fn serialize_i64(self, value: i64) -> Result<(), Error> {
        self.serialize_i128(value as i128)
    }

    /// Decimal text — the same digits `serde_json` writes with itoa, and unlike an integer *value*
    /// a 128-bit key has no `u64`/`i64` range limit to trip over.
    fn serialize_i128(self, value: i128) -> Result<(), Error> {
        self.encoder.push_key(&value.to_string());
        Ok(())
    }

    fn serialize_u8(self, value: u8) -> Result<(), Error> {
        self.serialize_u128(value as u128)
    }

    fn serialize_u16(self, value: u16) -> Result<(), Error> {
        self.serialize_u128(value as u128)
    }

    fn serialize_u32(self, value: u32) -> Result<(), Error> {
        self.serialize_u128(value as u128)
    }

    fn serialize_u64(self, value: u64) -> Result<(), Error> {
        self.serialize_u128(value as u128)
    }

    fn serialize_u128(self, value: u128) -> Result<(), Error> {
        self.encoder.push_key(&value.to_string());
        Ok(())
    }

    /// Rendered through `serde_json` itself, so a float key's shortest-round-trip text is by
    /// construction the text `serde_json` chose — note an `f32` key is formatted **as an `f32`**,
    /// unlike an `f32` *value*, which widens to `f64` first.
    fn serialize_f32(self, value: f32) -> Result<(), Error> {
        if !value.is_finite() {
            return Err(float_key_must_be_finite());
        }
        self.encoder.push_key(&serde_json::to_string(&value)?);
        Ok(())
    }

    fn serialize_f64(self, value: f64) -> Result<(), Error> {
        if !value.is_finite() {
            return Err(float_key_must_be_finite());
        }
        self.encoder.push_key(&serde_json::to_string(&value)?);
        Ok(())
    }

    fn serialize_char(self, value: char) -> Result<(), Error> {
        let mut buffer = [0u8; 4];
        self.encoder.push_key(value.encode_utf8(&mut buffer));
        Ok(())
    }

    fn serialize_str(self, value: &str) -> Result<(), Error> {
        self.encoder.push_key(value);
        Ok(())
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<(), Error> {
        self.encoder.push_key(variant);
        Ok(())
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        value.serialize(self)
    }

    fn collect_str<T: ?Sized + Display>(self, value: &T) -> Result<(), Error> {
        self.encoder.push_key(&value.to_string());
        Ok(())
    }

    fn serialize_bytes(self, _value: &[u8]) -> Result<(), Error> {
        Err(key_must_be_a_string())
    }

    fn serialize_none(self) -> Result<(), Error> {
        Err(key_must_be_a_string())
    }

    fn serialize_some<T: ?Sized + Serialize>(self, _value: &T) -> Result<(), Error> {
        Err(key_must_be_a_string())
    }

    fn serialize_unit(self) -> Result<(), Error> {
        Err(key_must_be_a_string())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<(), Error> {
        Err(key_must_be_a_string())
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<(), Error> {
        Err(key_must_be_a_string())
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Error> {
        Err(key_must_be_a_string())
    }

    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, Error> {
        Err(key_must_be_a_string())
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct, Error> {
        Err(key_must_be_a_string())
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, Error> {
        Err(key_must_be_a_string())
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Error> {
        Err(key_must_be_a_string())
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, Error> {
        Err(key_must_be_a_string())
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Error> {
        Err(key_must_be_a_string())
    }
}
