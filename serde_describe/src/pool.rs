use indexmap::IndexSet;
use serde::{Deserialize, Serialize};
use std::{
    borrow::Borrow,
    hash::{BuildHasher, BuildHasherDefault, Hash, Hasher},
    marker::PhantomData,
};

use crate::indices::{IndexIsEmpty, IsEmpty};

/// The hasher of the pools filled while tracing. Their values come from the traced types (names,
/// schema nodes, lists of indices), not from the traced values, so a fast non-cryptographic hash
/// is fine.
pub(crate) type FastState = BuildHasherDefault<FastHasher>;

/// FxHash-style hasher, see [`FastState`].
#[derive(Default, Clone, Copy)]
pub(crate) struct FastHasher(u64);

impl Hasher for FastHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let (words, rest) = bytes.as_chunks::<8>();
        for &word in words {
            self.write_u64(u64::from_le_bytes(word));
        }
        if !rest.is_empty() {
            let mut word = [0; 8];
            word[..rest.len()].copy_from_slice(rest);
            self.write_u64(u64::from_le_bytes(word));
        }
    }

    #[inline]
    fn write_u8(&mut self, value: u8) {
        self.write_u64(u64::from(value));
    }

    #[inline]
    fn write_u16(&mut self, value: u16) {
        self.write_u64(u64::from(value));
    }

    #[inline]
    fn write_u32(&mut self, value: u32) {
        self.write_u64(u64::from(value));
    }

    #[inline]
    fn write_u64(&mut self, value: u64) {
        self.0 = (self.0.rotate_left(5) ^ value).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }

    #[inline]
    fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }

    #[inline]
    fn write_isize(&mut self, value: isize) {
        self.write_u64(value as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

#[derive(Clone)]
pub(crate) struct Pool<ValueT, ValueIndexT> {
    inner: IndexSet<ValueT, FastState>,
    _dummy: PhantomData<ValueIndexT>,
}

impl<ValueT, ValueIndexT> Default for Pool<ValueT, ValueIndexT> {
    #[inline]
    fn default() -> Self {
        Self {
            inner: Default::default(),
            _dummy: PhantomData,
        }
    }
}

impl<ValueT, ValueIndexT> Pool<ValueT, ValueIndexT>
where
    ValueT: Hash + Eq + IsEmpty,
    ValueIndexT: TryFrom<usize> + IndexIsEmpty,
{
    pub(crate) fn intern(&mut self, value: ValueT) -> Result<ValueIndexT, ValueIndexT::Error> {
        if value.is_empty() {
            Ok(ValueIndexT::EMPTY)
        } else {
            ValueIndexT::try_from(self.inner.insert_full(value).0 + 1)
        }
    }

    pub(crate) fn intern_from<FromT>(
        &mut self,
        value: FromT,
    ) -> Result<ValueIndexT, ValueIndexT::Error>
    where
        ValueT: From<FromT>,
        FromT: IsEmpty,
    {
        if value.is_empty() {
            Ok(ValueIndexT::EMPTY)
        } else {
            ValueIndexT::try_from(self.inner.insert_full(value.into()).0 + 1)
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct NonEmptyPool<ValueT, ValueIndexT, HasherT = FastState> {
    inner: IndexSet<ValueT, HasherT>,
    _dummy: PhantomData<ValueIndexT>,
}

impl<ValueT, ValueIndexT, HasherT: Default> Default for NonEmptyPool<ValueT, ValueIndexT, HasherT> {
    #[inline]
    fn default() -> Self {
        Self {
            inner: Default::default(),
            _dummy: PhantomData,
        }
    }
}

impl<ValueT, ValueIndexT, HasherT> NonEmptyPool<ValueT, ValueIndexT, HasherT>
where
    ValueT: Hash + Eq,
    ValueIndexT: TryFrom<usize>,
    HasherT: BuildHasher,
{
    pub(crate) fn intern(&mut self, value: ValueT) -> Result<ValueIndexT, ValueIndexT::Error> {
        ValueIndexT::try_from(self.inner.insert_full(value).0)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct ReadonlyPool<ValueT, ValueIndexT> {
    values: Box<[ValueT]>,

    #[serde(skip)]
    _dummy: PhantomData<ValueIndexT>,
}

impl<ValueT, ValueIndexT> ReadonlyPool<ValueT, ValueIndexT>
where
    ValueT: IsEmpty,
    ValueIndexT: IsEmpty + Into<usize>,
{
    #[inline]
    pub(crate) fn get(&self, index: ValueIndexT) -> Option<&ValueT::Borrowed> {
        if index.is_empty() {
            Some(ValueT::BORROWED_EMPTY)
        } else {
            self.values.get(index.into() - 1).map(Borrow::borrow)
        }
    }

    /// The stored values, in index order starting at index 1 (index 0 is the empty value).
    #[inline]
    pub(crate) fn values(&self) -> &[ValueT] {
        &self.values
    }
}

impl<FromT, IntoT, ValueIndexT> From<Pool<FromT, ValueIndexT>> for ReadonlyPool<IntoT, ValueIndexT>
where
    FromT: Into<IntoT>,
{
    #[inline]
    fn from(value: Pool<FromT, ValueIndexT>) -> Self {
        Self {
            values: value.inner.into_iter().map(Into::into).collect(),
            _dummy: PhantomData,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct ReadonlyNonEmptyPool<ValueT, ValueIndexT> {
    values: Box<[ValueT]>,

    #[serde(skip)]
    _dummy: PhantomData<ValueIndexT>,
}

impl<ValueT, ValueIndexT> ReadonlyNonEmptyPool<ValueT, ValueIndexT>
where
    ValueIndexT: Into<usize>,
{
    #[inline]
    pub(crate) fn get(&self, index: ValueIndexT) -> Option<&ValueT> {
        self.values.get(index.into())
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.values.len()
    }
}

impl<FromT, IntoT, ValueIndexT, HasherT> From<NonEmptyPool<FromT, ValueIndexT, HasherT>>
    for ReadonlyNonEmptyPool<IntoT, ValueIndexT>
where
    FromT: Into<IntoT>,
{
    #[inline]
    fn from(value: NonEmptyPool<FromT, ValueIndexT, HasherT>) -> Self {
        Self {
            values: value.inner.into_iter().map(Into::into).collect(),
            _dummy: PhantomData,
        }
    }
}
