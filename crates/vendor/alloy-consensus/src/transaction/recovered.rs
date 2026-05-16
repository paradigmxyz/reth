use crate::crypto::RecoveryError;
use alloc::{borrow::Cow, vec::Vec};
use alloy_eips::{
    eip2718::{Encodable2718, WithEncoded},
    Typed2718,
};
use alloy_primitives::{bytes, Address, Bytes, Sealed, B256};
use alloy_rlp::{Decodable, Encodable};
use derive_more::{AsRef, Deref};

/// Signed object with recovered signer.
#[derive(Debug, Clone, Copy, PartialEq, Hash, Eq, AsRef, Deref)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Recovered<T> {
    /// Signer of the type
    signer: Address,
    /// Signed object
    #[deref]
    #[as_ref]
    #[cfg_attr(feature = "serde", serde(flatten))]
    inner: T,
}

impl<T> Recovered<T> {
    /// Signer of the object recovered from signature
    pub const fn signer(&self) -> Address {
        self.signer
    }

    /// Reference to the signer of the object recovered from signature
    pub const fn signer_ref(&self) -> &Address {
        &self.signer
    }

    /// Reference to the inner recovered object.
    pub const fn inner(&self) -> &T {
        &self.inner
    }

    /// Reference to the inner recovered object.
    pub const fn inner_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Reference to the inner signed object.
    pub fn into_inner(self) -> T {
        self.inner
    }

    /// Clone the inner signed object.
    pub fn clone_inner(&self) -> T
    where
        T: Clone,
    {
        self.inner.clone()
    }

    /// Dissolve Self to its component
    #[doc(alias = "split")]
    pub fn into_parts(self) -> (T, Address) {
        (self.inner, self.signer)
    }

    /// Converts from `&Recovered<T>` to `Recovered<&T>`.
    pub const fn as_recovered_ref(&self) -> Recovered<&T> {
        Recovered { inner: &self.inner, signer: self.signer() }
    }

    /// Create [`Recovered`] from the given transaction and [`Address`] of the signer.
    ///
    /// Note: This does not check if the signer is the actual signer of the transaction.
    #[inline]
    pub const fn new_unchecked(inner: T, signer: Address) -> Self {
        Self { inner, signer }
    }

    /// Converts the inner signed object to the given alternative that is `From<T>`
    pub fn convert<Tx>(self) -> Recovered<Tx>
    where
        Tx: From<T>,
    {
        self.map(Tx::from)
    }

    /// Converts the inner signed object to the given alternative that is `TryFrom<T>`
    pub fn try_convert<Tx>(self) -> Result<Recovered<Tx>, Tx::Error>
    where
        Tx: TryFrom<T>,
    {
        self.try_map(Tx::try_from)
    }

    /// Applies the given closure to the inner signed object.
    pub fn map<Tx>(self, f: impl FnOnce(T) -> Tx) -> Recovered<Tx> {
        Recovered::new_unchecked(f(self.inner), self.signer)
    }

    /// Applies the given fallible closure to the inner signed object.
    pub fn try_map<Tx, E>(self, f: impl FnOnce(T) -> Result<Tx, E>) -> Result<Recovered<Tx>, E> {
        Ok(Recovered::new_unchecked(f(self.inner)?, self.signer))
    }

    /// Returns the [`WithEncoded`] representation of [`Recovered`] with the given encoding.
    pub fn into_encoded_with(self, encoding: impl Into<Bytes>) -> WithEncoded<Self> {
        WithEncoded::new(encoding.into(), self)
    }

    /// Encodes the inner type and returns the [`WithEncoded`] representation of [`Recovered`].
    pub fn into_encoded(self) -> WithEncoded<Self>
    where
        T: Encodable2718,
    {
        let mut out = alloc::vec![];
        self.inner.encode_2718(&mut out);

        self.into_encoded_with(out)
    }
}

impl<T> Recovered<&T> {
    /// Maps a `Recovered<&T>` to a `Recovered<T>` by cloning the transaction.
    pub fn cloned(self) -> Recovered<T>
    where
        T: Clone,
    {
        let Self { inner, signer } = self;
        Recovered::new_unchecked(inner.clone(), signer)
    }

    /// Helper function to explicitly create a new copy of `Recovered<&T>`
    pub const fn copied(&self) -> Self {
        *self
    }
}

impl<T: Clone> Recovered<Cow<'_, T>> {
    /// Converts `Recovered<Cow<'_, T>>` into `Recovered<T>` by cloning the borrowed data if
    /// necessary.
    pub fn into_owned(self) -> Recovered<T> {
        let Self { inner, signer } = self;
        Recovered::new_unchecked(inner.into_owned(), signer)
    }
}

impl<T: Encodable> Encodable for Recovered<T> {
    /// This encodes the transaction _with_ the signature, and an rlp header.
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        self.inner.encode(out)
    }

    fn length(&self) -> usize {
        self.inner.length()
    }
}

impl<T: Decodable + SignerRecoverable> Decodable for Recovered<T> {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let tx = T::decode(buf)?;
        let signer = tx.recover_signer().map_err(|_| {
            alloy_rlp::Error::Custom("Unable to recover decoded transaction signer.")
        })?;
        Ok(Self::new_unchecked(tx, signer))
    }
}

impl<T: Typed2718> Typed2718 for Recovered<T> {
    fn ty(&self) -> u8 {
        self.inner.ty()
    }
}

impl<T: Encodable2718> Encodable2718 for Recovered<T> {
    fn encode_2718_len(&self) -> usize {
        self.inner.encode_2718_len()
    }

    fn encode_2718(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.inner.encode_2718(out)
    }

    fn trie_hash(&self) -> B256 {
        self.inner.trie_hash()
    }
}

impl<T> AsRef<Self> for Recovered<T> {
    fn as_ref(&self) -> &Self {
        self
    }
}

/// A type that can recover the signer of a transaction.
///
/// This is a helper trait that only provides the ability to recover the signer (address) of a
/// transaction.
pub trait SignerRecoverable {
    /// Recover signer from signature and hash.
    ///
    /// Returns an error if the transaction's signature is invalid following [EIP-2](https://eips.ethereum.org/EIPS/eip-2).
    ///
    /// Note:
    ///
    /// This can fail for some early ethereum mainnet transactions pre EIP-2, use
    /// [`Self::recover_signer_unchecked`] if you want to recover the signer without ensuring that
    /// the signature has a low `s` value.
    fn recover_signer(&self) -> Result<Address, RecoveryError>;

    /// Recover signer from signature and hash _without ensuring that the signature has a low `s`
    /// value_.
    ///
    /// Returns an error if the transaction's signature is invalid.
    fn recover_signer_unchecked(&self) -> Result<Address, RecoveryError>;

    /// Same as [`SignerRecoverable::recover_signer`] but receives a buffer to operate on
    /// for encoding. This is useful during batch recovery of transactions to avoid allocating a new
    /// buffer for each transaction.
    ///
    /// Caution: it is expected that implementations always clear this buffer before using it.
    fn recover_with_buf(&self, buf: &mut alloc::vec::Vec<u8>) -> Result<Address, RecoveryError> {
        let _ = buf;
        self.recover_signer()
    }

    /// Same as [`SignerRecoverable::recover_signer_unchecked`] but receives a buffer to operate on
    /// for encoding. This is useful during batch recovery of historical transactions to avoid
    /// allocating a new buffer for each transaction.
    ///
    /// Caution: it is expected that implementations always clear this buffer before using it.
    fn recover_unchecked_with_buf(
        &self,
        buf: &mut alloc::vec::Vec<u8>,
    ) -> Result<Address, RecoveryError> {
        let _ = buf;
        self.recover_signer()
    }

    /// Recover the signer via [`SignerRecoverable::recover_signer`] and returns a
    /// `Recovered<Self>`
    fn try_into_recovered(self) -> Result<Recovered<Self>, RecoveryError>
    where
        Self: Sized,
    {
        let signer = self.recover_signer()?;
        Ok(Recovered::new_unchecked(self, signer))
    }

    /// Recover the signer via [`SignerRecoverable::recover_signer_unchecked`] and returns a
    /// `Recovered<&Self>`
    fn try_into_recovered_unchecked(self) -> Result<Recovered<Self>, RecoveryError>
    where
        Self: Sized,
    {
        let signer = self.recover_signer_unchecked()?;
        Ok(Recovered::new_unchecked(self, signer))
    }

    /// Same as [`SignerRecoverable::try_into_recovered`] but receives a buffer to operate on
    /// for encoding. This is useful during batch recovery of transactions to avoid
    /// allocating a new buffer for each transaction.
    ///
    /// Caution: it is expected that implementations always clear this buffer before using it.
    fn try_into_recovered_with_buf(
        self,
        buf: &mut alloc::vec::Vec<u8>,
    ) -> Result<Recovered<Self>, RecoveryError>
    where
        Self: Sized,
    {
        let signer = self.recover_with_buf(buf)?;
        Ok(Recovered::new_unchecked(self, signer))
    }
    /// Same as [`SignerRecoverable::try_into_recovered_unchecked`] but receives a buffer to operate
    /// on for encoding. This is useful during batch recovery of historical transactions to
    /// avoid allocating a new buffer for each transaction.
    ///
    /// Caution: it is expected that implementations always clear this buffer before using it.
    fn try_into_recovered_unchecked_with_buf(
        self,
        buf: &mut alloc::vec::Vec<u8>,
    ) -> Result<Recovered<Self>, RecoveryError>
    where
        Self: Sized,
    {
        let signer = self.recover_unchecked_with_buf(buf)?;
        Ok(Recovered::new_unchecked(self, signer))
    }

    /// Recover the signer via [`SignerRecoverable::recover_signer`] and returns a
    /// `Recovered<&Self>`
    fn try_to_recovered_ref(&self) -> Result<Recovered<&Self>, RecoveryError> {
        let signer = self.recover_signer()?;
        Ok(Recovered::new_unchecked(self, signer))
    }

    /// Same as [`SignerRecoverable::try_to_recovered_ref`] but receives a buffer to operate on
    /// for encoding. This is useful during batch recovery of transactions to avoid
    /// allocating a new buffer for each transaction.
    ///
    /// Caution: it is expected that implementations always clear this buffer before using it.
    fn try_to_recovered_ref_with_buf(
        &self,
        buf: &mut alloc::vec::Vec<u8>,
    ) -> Result<Recovered<&Self>, RecoveryError> {
        let signer = self.recover_with_buf(buf)?;
        Ok(Recovered::new_unchecked(self, signer))
    }

    /// Recover the signer via [`SignerRecoverable::recover_signer_unchecked`] and returns a
    /// `Recovered<&Self>`
    fn try_to_recovered_ref_unchecked(&self) -> Result<Recovered<&Self>, RecoveryError> {
        let signer = self.recover_signer_unchecked()?;
        Ok(Recovered::new_unchecked(self, signer))
    }

    /// Same as [`SignerRecoverable::try_to_recovered_ref_unchecked`] but receives a buffer to
    /// operate on for encoding. This is useful during batch recovery of historical transactions
    /// to avoid allocating a new buffer for each transaction.
    ///
    /// Caution: it is expected that implementations always clear this buffer before using it.
    fn try_to_recovered_ref_unchecked_with_buf(
        &self,
        buf: &mut alloc::vec::Vec<u8>,
    ) -> Result<Recovered<&Self>, RecoveryError> {
        let signer = self.recover_unchecked_with_buf(buf)?;
        Ok(Recovered::new_unchecked(self, signer))
    }
}

impl<T> SignerRecoverable for WithEncoded<T>
where
    T: SignerRecoverable,
{
    fn recover_signer(&self) -> Result<Address, RecoveryError> {
        self.1.recover_signer()
    }

    fn recover_signer_unchecked(&self) -> Result<Address, RecoveryError> {
        self.1.recover_signer_unchecked()
    }

    fn recover_with_buf(&self, buf: &mut Vec<u8>) -> Result<Address, RecoveryError> {
        self.1.recover_with_buf(buf)
    }

    fn recover_unchecked_with_buf(
        &self,
        buf: &mut alloc::vec::Vec<u8>,
    ) -> Result<Address, RecoveryError> {
        self.1.recover_unchecked_with_buf(buf)
    }
}

impl<T> SignerRecoverable for Sealed<T>
where
    T: SignerRecoverable,
{
    fn recover_signer(&self) -> Result<Address, RecoveryError> {
        self.inner().recover_signer()
    }

    fn recover_signer_unchecked(&self) -> Result<Address, RecoveryError> {
        self.inner().recover_signer_unchecked()
    }

    fn recover_with_buf(&self, buf: &mut alloc::vec::Vec<u8>) -> Result<Address, RecoveryError> {
        self.inner().recover_with_buf(buf)
    }

    fn recover_unchecked_with_buf(
        &self,
        buf: &mut alloc::vec::Vec<u8>,
    ) -> Result<Address, RecoveryError> {
        self.inner().recover_unchecked_with_buf(buf)
    }
}
