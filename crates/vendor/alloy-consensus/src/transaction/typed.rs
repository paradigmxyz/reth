pub use crate::transaction::envelope::EthereumTypedTransaction;
use crate::{
    error::ValueError,
    private::alloy_eips::eip2718::Eip2718Error,
    transaction::{
        eip4844::{TxEip4844, TxEip4844Variant, TxEip4844WithSidecar},
        RlpEcdsaDecodableTx, RlpEcdsaEncodableTx,
    },
    EthereumTxEnvelope, SignableTransaction, Signed, TxEip1559, TxEip2930, TxEip7702, TxLegacy,
    TxType,
};
use alloy_eips::{eip2718::Eip2718Result, Typed2718};
use alloy_primitives::{ChainId, Signature, TxHash};
use alloy_rlp::{Buf, BufMut, Decodable};

/// Basic typed transaction which can contain both [`TxEip4844`] and [`TxEip4844WithSidecar`].
pub type TypedTransaction = EthereumTypedTransaction<TxEip4844Variant>;

impl<Eip4844> From<TxLegacy> for EthereumTypedTransaction<Eip4844> {
    fn from(tx: TxLegacy) -> Self {
        Self::Legacy(tx)
    }
}

impl<Eip4844> From<TxEip2930> for EthereumTypedTransaction<Eip4844> {
    fn from(tx: TxEip2930) -> Self {
        Self::Eip2930(tx)
    }
}

impl<Eip4844> From<TxEip1559> for EthereumTypedTransaction<Eip4844> {
    fn from(tx: TxEip1559) -> Self {
        Self::Eip1559(tx)
    }
}

impl<Eip4844: From<TxEip4844>> From<TxEip4844> for EthereumTypedTransaction<Eip4844> {
    fn from(tx: TxEip4844) -> Self {
        Self::Eip4844(tx.into())
    }
}

impl<T, Eip4844: From<TxEip4844WithSidecar<T>>> From<TxEip4844WithSidecar<T>>
    for EthereumTypedTransaction<Eip4844>
{
    fn from(tx: TxEip4844WithSidecar<T>) -> Self {
        Self::Eip4844(tx.into())
    }
}

impl<Sidecar, Eip4844: From<TxEip4844Variant<Sidecar>>> From<TxEip4844Variant<Sidecar>>
    for EthereumTypedTransaction<Eip4844>
{
    fn from(tx: TxEip4844Variant<Sidecar>) -> Self {
        Self::Eip4844(tx.into())
    }
}

impl<Eip4844> From<TxEip7702> for EthereumTypedTransaction<Eip4844> {
    fn from(tx: TxEip7702) -> Self {
        Self::Eip7702(tx)
    }
}

impl<Eip4844> From<EthereumTxEnvelope<Eip4844>> for EthereumTypedTransaction<Eip4844> {
    fn from(envelope: EthereumTxEnvelope<Eip4844>) -> Self {
        match envelope {
            EthereumTxEnvelope::Legacy(tx) => Self::Legacy(tx.strip_signature()),
            EthereumTxEnvelope::Eip2930(tx) => Self::Eip2930(tx.strip_signature()),
            EthereumTxEnvelope::Eip1559(tx) => Self::Eip1559(tx.strip_signature()),
            EthereumTxEnvelope::Eip4844(tx) => Self::Eip4844(tx.strip_signature()),
            EthereumTxEnvelope::Eip7702(tx) => Self::Eip7702(tx.strip_signature()),
        }
    }
}

impl<T> From<EthereumTypedTransaction<TxEip4844WithSidecar<T>>>
    for EthereumTypedTransaction<TxEip4844>
{
    fn from(value: EthereumTypedTransaction<TxEip4844WithSidecar<T>>) -> Self {
        value.map_eip4844(|eip4844| eip4844.into())
    }
}

impl<T> From<EthereumTypedTransaction<TxEip4844Variant<T>>>
    for EthereumTypedTransaction<TxEip4844>
{
    fn from(value: EthereumTypedTransaction<TxEip4844Variant<T>>) -> Self {
        value.map_eip4844(|eip4844| eip4844.into())
    }
}

impl<T> From<EthereumTypedTransaction<TxEip4844>>
    for EthereumTypedTransaction<TxEip4844Variant<T>>
{
    fn from(value: EthereumTypedTransaction<TxEip4844>) -> Self {
        value.map_eip4844(|eip4844| eip4844.into())
    }
}

impl<Eip4844> EthereumTypedTransaction<Eip4844> {
    /// Converts the EIP-4844 variant of this transaction with the given closure.
    ///
    /// This is intended to convert between the EIP-4844 variants, specifically for stripping away
    /// non consensus data (blob sidecar data).
    pub fn map_eip4844<U>(self, mut f: impl FnMut(Eip4844) -> U) -> EthereumTypedTransaction<U> {
        match self {
            Self::Legacy(tx) => EthereumTypedTransaction::Legacy(tx),
            Self::Eip2930(tx) => EthereumTypedTransaction::Eip2930(tx),
            Self::Eip1559(tx) => EthereumTypedTransaction::Eip1559(tx),
            Self::Eip4844(tx) => EthereumTypedTransaction::Eip4844(f(tx)),
            Self::Eip7702(tx) => EthereumTypedTransaction::Eip7702(tx),
        }
    }

    /// Converts this typed transaction into a signed [`EthereumTxEnvelope`]
    pub fn into_envelope(self, signature: Signature) -> EthereumTxEnvelope<Eip4844> {
        match self {
            Self::Legacy(tx) => EthereumTxEnvelope::Legacy(tx.into_signed(signature)),
            Self::Eip2930(tx) => EthereumTxEnvelope::Eip2930(tx.into_signed(signature)),
            Self::Eip1559(tx) => EthereumTxEnvelope::Eip1559(tx.into_signed(signature)),
            Self::Eip4844(tx) => EthereumTxEnvelope::Eip4844(Signed::new_unhashed(tx, signature)),
            Self::Eip7702(tx) => EthereumTxEnvelope::Eip7702(tx.into_signed(signature)),
        }
    }
}

impl<Eip4844: RlpEcdsaDecodableTx> EthereumTypedTransaction<Eip4844> {
    /// Decode an unsigned typed transaction from RLP bytes.
    pub fn decode_unsigned(buf: &mut &[u8]) -> Eip2718Result<Self> {
        if buf.is_empty() {
            return Err(alloy_rlp::Error::InputTooShort.into());
        }

        let first_byte = buf[0];

        // Eip2718: legacy transactions start with >= 0xc0
        if first_byte >= 0xc0 {
            return Ok(Self::Legacy(TxLegacy::decode(buf)?));
        }

        let tx_type = buf.get_u8();
        match tx_type {
            0x00 => Ok(Self::Legacy(TxLegacy::decode(buf)?)),
            0x01 => Ok(Self::Eip2930(TxEip2930::decode(buf)?)),
            0x02 => Ok(Self::Eip1559(TxEip1559::decode(buf)?)),
            0x03 => Ok(Self::Eip4844(Eip4844::rlp_decode(buf)?)),
            0x04 => Ok(Self::Eip7702(TxEip7702::decode(buf)?)),
            _ => Err(Eip2718Error::UnexpectedType(tx_type)),
        }
    }
}

impl<Eip4844: RlpEcdsaEncodableTx> EthereumTypedTransaction<Eip4844> {
    /// Return the [`TxType`] of the inner txn.
    #[doc(alias = "transaction_type")]
    pub const fn tx_type(&self) -> TxType {
        match self {
            Self::Legacy(_) => TxType::Legacy,
            Self::Eip2930(_) => TxType::Eip2930,
            Self::Eip1559(_) => TxType::Eip1559,
            Self::Eip4844(_) => TxType::Eip4844,
            Self::Eip7702(_) => TxType::Eip7702,
        }
    }

    /// Return the inner legacy transaction if it exists.
    pub const fn legacy(&self) -> Option<&TxLegacy> {
        match self {
            Self::Legacy(tx) => Some(tx),
            _ => None,
        }
    }

    /// Return the inner EIP-2930 transaction if it exists.
    pub const fn eip2930(&self) -> Option<&TxEip2930> {
        match self {
            Self::Eip2930(tx) => Some(tx),
            _ => None,
        }
    }

    /// Return the inner EIP-1559 transaction if it exists.
    pub const fn eip1559(&self) -> Option<&TxEip1559> {
        match self {
            Self::Eip1559(tx) => Some(tx),
            _ => None,
        }
    }

    /// Return the inner EIP-7702 transaction if it exists.
    pub const fn eip7702(&self) -> Option<&TxEip7702> {
        match self {
            Self::Eip7702(tx) => Some(tx),
            _ => None,
        }
    }

    /// Consumes the type and returns the [`TxLegacy`] if this transaction is of that type.
    pub fn try_into_legacy(self) -> Result<TxLegacy, ValueError<Self>> {
        match self {
            Self::Legacy(tx) => Ok(tx),
            _ => Err(ValueError::new(self, "Expected legacy transaction")),
        }
    }

    /// Consumes the type and returns the [`TxEip2930`]if this transaction is of that type.
    pub fn try_into_eip2930(self) -> Result<TxEip2930, ValueError<Self>> {
        match self {
            Self::Eip2930(tx) => Ok(tx),
            _ => Err(ValueError::new(self, "Expected EIP-2930 transaction")),
        }
    }

    /// Consumes the type and returns the EIP-4844 if this transaction is of that type.
    pub fn try_into_eip4844(self) -> Result<Eip4844, ValueError<Self>> {
        match self {
            Self::Eip4844(tx) => Ok(tx),
            _ => Err(ValueError::new(self, "Expected EIP-4844 transaction")),
        }
    }

    /// Consumes the type and returns the EIP-7702 if this transaction is of that type.
    pub fn try_into_eip7702(self) -> Result<TxEip7702, ValueError<Self>> {
        match self {
            Self::Eip7702(tx) => Ok(tx),
            _ => Err(ValueError::new(self, "Expected EIP-7702 transaction")),
        }
    }

    /// Calculate the transaction hash for the given signature.
    pub fn tx_hash(&self, signature: &Signature) -> TxHash {
        match self {
            Self::Legacy(tx) => tx.tx_hash(signature),
            Self::Eip2930(tx) => tx.tx_hash(signature),
            Self::Eip1559(tx) => tx.tx_hash(signature),
            Self::Eip4844(tx) => tx.tx_hash(signature),
            Self::Eip7702(tx) => tx.tx_hash(signature),
        }
    }
}

impl<Eip4844: RlpEcdsaEncodableTx + Typed2718> RlpEcdsaEncodableTx
    for EthereumTypedTransaction<Eip4844>
{
    fn rlp_encoded_fields_length(&self) -> usize {
        match self {
            Self::Legacy(tx) => tx.rlp_encoded_fields_length(),
            Self::Eip2930(tx) => tx.rlp_encoded_fields_length(),
            Self::Eip1559(tx) => tx.rlp_encoded_fields_length(),
            Self::Eip4844(tx) => tx.rlp_encoded_fields_length(),
            Self::Eip7702(tx) => tx.rlp_encoded_fields_length(),
        }
    }

    fn rlp_encode_fields(&self, out: &mut dyn alloy_rlp::BufMut) {
        match self {
            Self::Legacy(tx) => tx.rlp_encode_fields(out),
            Self::Eip2930(tx) => tx.rlp_encode_fields(out),
            Self::Eip1559(tx) => tx.rlp_encode_fields(out),
            Self::Eip4844(tx) => tx.rlp_encode_fields(out),
            Self::Eip7702(tx) => tx.rlp_encode_fields(out),
        }
    }

    fn eip2718_encode_with_type(&self, signature: &Signature, _ty: u8, out: &mut dyn BufMut) {
        match self {
            Self::Legacy(tx) => tx.eip2718_encode_with_type(signature, tx.ty(), out),
            Self::Eip2930(tx) => tx.eip2718_encode_with_type(signature, tx.ty(), out),
            Self::Eip1559(tx) => tx.eip2718_encode_with_type(signature, tx.ty(), out),
            Self::Eip4844(tx) => tx.eip2718_encode_with_type(signature, tx.ty(), out),
            Self::Eip7702(tx) => tx.eip2718_encode_with_type(signature, tx.ty(), out),
        }
    }

    fn eip2718_encode(&self, signature: &Signature, out: &mut dyn BufMut) {
        match self {
            Self::Legacy(tx) => tx.eip2718_encode(signature, out),
            Self::Eip2930(tx) => tx.eip2718_encode(signature, out),
            Self::Eip1559(tx) => tx.eip2718_encode(signature, out),
            Self::Eip4844(tx) => tx.eip2718_encode(signature, out),
            Self::Eip7702(tx) => tx.eip2718_encode(signature, out),
        }
    }

    fn network_encode_with_type(&self, signature: &Signature, _ty: u8, out: &mut dyn BufMut) {
        match self {
            Self::Legacy(tx) => tx.network_encode_with_type(signature, tx.ty(), out),
            Self::Eip2930(tx) => tx.network_encode_with_type(signature, tx.ty(), out),
            Self::Eip1559(tx) => tx.network_encode_with_type(signature, tx.ty(), out),
            Self::Eip4844(tx) => tx.network_encode_with_type(signature, tx.ty(), out),
            Self::Eip7702(tx) => tx.network_encode_with_type(signature, tx.ty(), out),
        }
    }

    fn network_encode(&self, signature: &Signature, out: &mut dyn BufMut) {
        match self {
            Self::Legacy(tx) => tx.network_encode(signature, out),
            Self::Eip2930(tx) => tx.network_encode(signature, out),
            Self::Eip1559(tx) => tx.network_encode(signature, out),
            Self::Eip4844(tx) => tx.network_encode(signature, out),
            Self::Eip7702(tx) => tx.network_encode(signature, out),
        }
    }

    fn tx_hash_with_type(&self, signature: &Signature, _ty: u8) -> TxHash {
        match self {
            Self::Legacy(tx) => tx.tx_hash_with_type(signature, tx.ty()),
            Self::Eip2930(tx) => tx.tx_hash_with_type(signature, tx.ty()),
            Self::Eip1559(tx) => tx.tx_hash_with_type(signature, tx.ty()),
            Self::Eip4844(tx) => tx.tx_hash_with_type(signature, tx.ty()),
            Self::Eip7702(tx) => tx.tx_hash_with_type(signature, tx.ty()),
        }
    }

    fn tx_hash(&self, signature: &Signature) -> TxHash {
        match self {
            Self::Legacy(tx) => tx.tx_hash(signature),
            Self::Eip2930(tx) => tx.tx_hash(signature),
            Self::Eip1559(tx) => tx.tx_hash(signature),
            Self::Eip4844(tx) => tx.tx_hash(signature),
            Self::Eip7702(tx) => tx.tx_hash(signature),
        }
    }
}

impl<Eip4844: SignableTransaction<Signature>> SignableTransaction<Signature>
    for EthereumTypedTransaction<Eip4844>
{
    fn set_chain_id(&mut self, chain_id: ChainId) {
        match self {
            Self::Legacy(tx) => tx.set_chain_id(chain_id),
            Self::Eip2930(tx) => tx.set_chain_id(chain_id),
            Self::Eip1559(tx) => tx.set_chain_id(chain_id),
            Self::Eip4844(tx) => tx.set_chain_id(chain_id),
            Self::Eip7702(tx) => tx.set_chain_id(chain_id),
        }
    }

    fn encode_for_signing(&self, out: &mut dyn BufMut) {
        match self {
            Self::Legacy(tx) => tx.encode_for_signing(out),
            Self::Eip2930(tx) => tx.encode_for_signing(out),
            Self::Eip1559(tx) => tx.encode_for_signing(out),
            Self::Eip4844(tx) => tx.encode_for_signing(out),
            Self::Eip7702(tx) => tx.encode_for_signing(out),
        }
    }

    fn payload_len_for_signature(&self) -> usize {
        match self {
            Self::Legacy(tx) => tx.payload_len_for_signature(),
            Self::Eip2930(tx) => tx.payload_len_for_signature(),
            Self::Eip1559(tx) => tx.payload_len_for_signature(),
            Self::Eip4844(tx) => tx.payload_len_for_signature(),
            Self::Eip7702(tx) => tx.payload_len_for_signature(),
        }
    }
}

#[cfg(feature = "serde")]
impl<Eip4844, T: From<EthereumTypedTransaction<Eip4844>>> From<EthereumTypedTransaction<Eip4844>>
    for alloy_serde::WithOtherFields<T>
{
    fn from(value: EthereumTypedTransaction<Eip4844>) -> Self {
        Self::new(value.into())
    }
}

#[cfg(feature = "serde")]
impl<Eip4844, T> From<EthereumTxEnvelope<Eip4844>> for alloy_serde::WithOtherFields<T>
where
    T: From<EthereumTxEnvelope<Eip4844>>,
{
    fn from(value: EthereumTxEnvelope<Eip4844>) -> Self {
        Self::new(value.into())
    }
}

/// Bincode-compatible [`EthereumTypedTransaction`] serde implementation.
#[cfg(all(feature = "serde", feature = "serde-bincode-compat"))]
pub(crate) mod serde_bincode_compat {
    use alloc::borrow::Cow;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_with::{DeserializeAs, SerializeAs};

    /// Bincode-compatible [`super::EthereumTypedTransaction`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use alloy_consensus::{serde_bincode_compat, EthereumTypedTransaction};
    /// use serde::{de::DeserializeOwned, Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data<T: Serialize + DeserializeOwned + Clone + 'static> {
    ///     #[serde_as(as = "serde_bincode_compat::EthereumTypedTransaction<'_, T>")]
    ///     receipt: EthereumTypedTransaction<T>,
    /// }
    /// ```
    #[derive(Debug, Serialize, Deserialize)]
    pub enum EthereumTypedTransaction<'a, Eip4844: Clone = crate::transaction::TxEip4844> {
        /// Legacy transaction
        Legacy(crate::serde_bincode_compat::transaction::TxLegacy<'a>),
        /// EIP-2930 transaction
        Eip2930(crate::serde_bincode_compat::transaction::TxEip2930<'a>),
        /// EIP-1559 transaction
        Eip1559(crate::serde_bincode_compat::transaction::TxEip1559<'a>),
        /// EIP-4844 transaction
        /// Note: assumes EIP4844 is bincode compatible, which it is because no flatten or skipped
        /// fields.
        Eip4844(Cow<'a, Eip4844>),
        /// EIP-7702 transaction
        Eip7702(crate::serde_bincode_compat::transaction::TxEip7702<'a>),
    }

    impl<'a, T: Clone> From<&'a super::EthereumTypedTransaction<T>>
        for EthereumTypedTransaction<'a, T>
    {
        fn from(value: &'a super::EthereumTypedTransaction<T>) -> Self {
            match value {
                super::EthereumTypedTransaction::Legacy(tx) => Self::Legacy(tx.into()),
                super::EthereumTypedTransaction::Eip2930(tx) => Self::Eip2930(tx.into()),
                super::EthereumTypedTransaction::Eip1559(tx) => Self::Eip1559(tx.into()),
                super::EthereumTypedTransaction::Eip4844(tx) => Self::Eip4844(Cow::Borrowed(tx)),
                super::EthereumTypedTransaction::Eip7702(tx) => Self::Eip7702(tx.into()),
            }
        }
    }

    impl<'a, T: Clone> From<EthereumTypedTransaction<'a, T>> for super::EthereumTypedTransaction<T> {
        fn from(value: EthereumTypedTransaction<'a, T>) -> Self {
            match value {
                EthereumTypedTransaction::Legacy(tx) => Self::Legacy(tx.into()),
                EthereumTypedTransaction::Eip2930(tx) => Self::Eip2930(tx.into()),
                EthereumTypedTransaction::Eip1559(tx) => Self::Eip1559(tx.into()),
                EthereumTypedTransaction::Eip4844(tx) => Self::Eip4844(tx.into_owned()),
                EthereumTypedTransaction::Eip7702(tx) => Self::Eip7702(tx.into()),
            }
        }
    }

    impl<T: Serialize + Clone> SerializeAs<super::EthereumTypedTransaction<T>>
        for EthereumTypedTransaction<'_, T>
    {
        fn serialize_as<S>(
            source: &super::EthereumTypedTransaction<T>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            EthereumTypedTransaction::<'_, T>::from(source).serialize(serializer)
        }
    }

    impl<'de, T: Deserialize<'de> + Clone> DeserializeAs<'de, super::EthereumTypedTransaction<T>>
        for EthereumTypedTransaction<'de, T>
    {
        fn deserialize_as<D>(
            deserializer: D,
        ) -> Result<super::EthereumTypedTransaction<T>, D::Error>
        where
            D: Deserializer<'de>,
        {
            EthereumTypedTransaction::<'_, T>::deserialize(deserializer).map(Into::into)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::super::{serde_bincode_compat, EthereumTypedTransaction};
        use crate::TxEip4844;
        use arbitrary::Arbitrary;
        use bincode::config;
        use rand::Rng;
        use serde::{Deserialize, Serialize};
        use serde_with::serde_as;

        #[test]
        fn test_typed_tx_bincode_roundtrip() {
            #[serde_as]
            #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
            struct Data {
                #[serde_as(as = "serde_bincode_compat::EthereumTypedTransaction<'_>")]
                transaction: EthereumTypedTransaction<TxEip4844>,
            }

            let mut bytes = [0u8; 1024];
            rand::thread_rng().fill(bytes.as_mut_slice());
            let data = Data {
                transaction: EthereumTypedTransaction::arbitrary(
                    &mut arbitrary::Unstructured::new(&bytes),
                )
                .unwrap(),
            };

            let encoded = bincode::serde::encode_to_vec(&data, config::legacy()).unwrap();
            let (decoded, _) =
                bincode::serde::decode_from_slice::<Data, _>(&encoded, config::legacy()).unwrap();
            assert_eq!(decoded, data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip2930::AccessList;
    use alloy_primitives::{Address, Bytes, U256};

    #[test]
    fn test_decode_unsigned_all_types() {
        let transactions = [
            TypedTransaction::Legacy(TxLegacy {
                chain_id: Some(1),
                nonce: 0,
                gas_price: 1,
                gas_limit: 2,
                to: Address::ZERO.into(),
                value: U256::ZERO,
                input: Bytes::default(),
            }),
            TypedTransaction::Eip2930(TxEip2930 {
                chain_id: 1,
                nonce: 0,
                gas_price: 1,
                gas_limit: 2,
                to: Address::ZERO.into(),
                value: U256::ZERO,
                input: Bytes::default(),
                access_list: AccessList::default(),
            }),
            TypedTransaction::Eip1559(TxEip1559 {
                chain_id: 1,
                nonce: 0,
                gas_limit: 1,
                max_fee_per_gas: 2,
                max_priority_fee_per_gas: 3,
                to: Address::ZERO.into(),
                value: U256::ZERO,
                input: Bytes::default(),
                access_list: AccessList::default(),
            }),
            TypedTransaction::Eip7702(TxEip7702 {
                chain_id: 1,
                nonce: 0,
                gas_limit: 1,
                max_fee_per_gas: 2,
                max_priority_fee_per_gas: 3,
                to: Address::ZERO,
                value: U256::ZERO,
                input: Bytes::default(),
                access_list: AccessList::default(),
                authorization_list: vec![],
            }),
        ];

        for tx in transactions {
            let mut encoded = Vec::new();
            tx.encode_for_signing(&mut encoded);
            let decoded = TypedTransaction::decode_unsigned(&mut encoded.as_slice()).unwrap();
            assert_eq!(decoded, tx);
        }
    }

    #[test]
    fn test_decode_unsigned_invalid_type() {
        let invalid = vec![0x99, 0xc0];
        let result = TypedTransaction::decode_unsigned(&mut invalid.as_slice());
        assert!(result.is_err());
        if let Err(err) = result {
            assert!(matches!(err, alloy_eips::eip2718::Eip2718Error::UnexpectedType(0x99)));
        }
    }

    #[test]
    fn test_decode_unsigned_encode_stability() {
        // Verify that decode(encode(tx)) == tx and encode(decode(encode(tx))) == encode(tx)
        let tx = TypedTransaction::Eip1559(TxEip1559 {
            chain_id: 1,
            nonce: 100,
            gas_limit: 50000,
            max_fee_per_gas: 30_000_000_000,
            max_priority_fee_per_gas: 2_000_000_000,
            to: Address::random().into(),
            value: U256::from(1_000_000),
            input: Bytes::from(vec![1, 2, 3]),
            access_list: AccessList::default(),
        });

        // Encode
        let mut encoded = Vec::new();
        tx.encode_for_signing(&mut encoded);

        // Decode
        let decoded = TypedTransaction::decode_unsigned(&mut encoded.as_slice()).unwrap();
        assert_eq!(decoded, tx);

        // Re-encode
        let mut re_encoded = Vec::new();
        decoded.encode_for_signing(&mut re_encoded);

        assert_eq!(encoded, re_encoded);
    }
}
