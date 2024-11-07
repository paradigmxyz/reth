use alloy_consensus::{
    transaction::{from_eip155_value, RlpEcdsaTx},
    Header, TxEip1559, TxEip2930, TxEip4844, TxEip7702, TxLegacy,
};
use alloy_eips::{
    eip2718::{Decodable2718, Eip2718Error, Eip2718Result, Encodable2718},
    eip2930::AccessList,
    eip7702::SignedAuthorization,
};
use alloy_primitives::{
    bytes::{Buf, BufMut, BytesMut},
    keccak256, Address, Bytes, ChainId, PrimitiveSignature as Signature, TxHash, TxKind, B256,
    U256,
};
use alloy_rlp::{Decodable, Encodable, Error as RlpError, RlpDecodable, RlpEncodable};
use core::mem;
use derive_more::{AsRef, Deref};
use op_alloy_consensus::TxDeposit;
use rayon::prelude::{IntoParallelIterator, ParallelIterator};
use reth_downloaders::file_client::FileClientError;
use reth_primitives::{
    transaction::{
        signature::{recover_signer, recover_signer_unchecked},
        Transaction, TxType, PARALLEL_SENDER_RECOVERY_THRESHOLD,
    },
    Withdrawals,
};
use serde::{Deserialize, Serialize};
use tokio_util::codec::{Decoder, Encoder};

#[allow(dead_code)]
/// Specific codec for reading raw block bodies from a file
/// with optimism-specific signature handling
pub(crate) struct OvmBlockFileCodec;

impl Decoder for OvmBlockFileCodec {
    type Item = Block;
    type Error = FileClientError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.is_empty() {
            return Ok(None);
        }

        let buf_slice = &mut src.as_ref();
        let body =
            Block::decode(buf_slice).map_err(|err| FileClientError::Rlp(err, src.to_vec()))?;
        src.advance(src.len() - buf_slice.len());

        Ok(Some(body))
    }
}

impl Encoder<Block> for OvmBlockFileCodec {
    type Error = FileClientError;

    fn encode(&mut self, item: Block, dst: &mut BytesMut) -> Result<(), Self::Error> {
        item.encode(dst);
        Ok(())
    }
}

/// OVM block, same as EVM block but with different transaction signature handling
/// Pre-bedrock system transactions on Optimism were sent from the zero address
/// with an empty signature,
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
pub struct Block {
    /// Block header
    pub header: Header,
    /// Block body
    pub body: BlockBody,
}

impl Block {
    /// Decodes a `Block` from the given byte slice.
    pub fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let header = Header::decode(buf)?;
        let body = BlockBody::decode(buf)?;
        Ok(Self { header, body })
    }
    /// Encodes the `Block` into the `out` buffer.
    pub fn encode(&self, out: &mut dyn BufMut) {
        self.header.encode(out);
        self.body.encode(out);
    }
}

/// The body of a block for OVM
#[derive(Debug, Clone, PartialEq, Eq, Default, RlpEncodable, RlpDecodable)]
#[rlp(trailing)]
pub struct BlockBody {
    /// Transactions in the block
    pub transactions: Vec<TransactionSigned>,
    /// Uncle headers for the given block
    pub ommers: Vec<Header>,
    /// Withdrawals in the block.
    pub withdrawals: Option<Withdrawals>,
}

/// Signed transaction.
#[derive(Debug, Clone, PartialEq, Eq, Hash, AsRef, Deref, Serialize, Deserialize)]
pub struct TransactionSigned {
    /// Transaction hash
    pub hash: TxHash,
    /// The transaction signature values
    pub signature: Signature,
    /// Raw transaction info
    #[deref]
    #[as_ref]
    pub transaction: Transaction,
}

impl Default for TransactionSigned {
    fn default() -> Self {
        Self {
            hash: Default::default(),
            signature: Signature::test_signature(),
            transaction: Default::default(),
        }
    }
}

impl AsRef<Self> for TransactionSigned {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl From<Block> for BlockBody {
    fn from(block: Block) -> Self {
        Self {
            transactions: block.body.transactions.into_iter().collect(),
            ommers: block.body.ommers.into_iter().collect(),
            withdrawals: block.body.withdrawals,
        }
    }
}

// === impl TransactionSigned ===

impl TransactionSigned {
    /// Transaction signature.
    pub const fn signature(&self) -> &Signature {
        &self.signature
    }

    /// Transaction hash. Used to identify transaction.
    pub const fn hash(&self) -> TxHash {
        self.hash
    }

    /// Reference to transaction hash. Used to identify transaction.
    pub const fn hash_ref(&self) -> &TxHash {
        &self.hash
    }

    /// Recover signer from signature and hash.
    ///
    /// Returns `None` if the transaction's signature is invalid following [EIP-2](https://eips.ethereum.org/EIPS/eip-2), see also [`recover_signer`].
    ///
    /// Note:
    ///
    /// This can fail for some early ethereum mainnet transactions pre EIP-2, use
    /// [`Self::recover_signer_unchecked`] if you want to recover the signer without ensuring that
    /// the signature has a low `s` value.
    /// Optimism's Deposit transaction does not have a signature
    pub fn recover_signer(&self) -> Option<Address> {
        // For deposit transactions, directly return `from`the ` address
        if let Transaction::Deposit(TxDeposit { from, .. }) = self.transaction {
            return Some(from);
        }
        // For pre-bedrock system transactions with empty signatures, return zero address
        if self.is_legacy() && self.signature == TxDeposit::signature() {
            return Some(Address::ZERO);
        }

        let signature_hash = self.signature_hash();
        recover_signer(&self.signature, signature_hash)
    }

    /// Recover signer from signature and hash _without ensuring that the signature has a low `s`
    /// value_.
    ///
    /// Returns `None` if the transaction's signature is invalid, see also
    /// [`recover_signer_unchecked`].
    pub fn recover_signer_unchecked(&self) -> Option<Address> {
        // Optimism's Deposit transaction does not have a signature. Directly return the
        // `from` address.
        #[cfg(feature = "optimism")]
        if let Transaction::Deposit(TxDeposit { from, .. }) = self.transaction {
            return Some(from);
        }
        let signature_hash = self.signature_hash();
        recover_signer_unchecked(&self.signature, signature_hash)
    }

    /// Recovers a list of signers from a transaction list iterator.
    ///
    /// Returns `None`, if some transaction's signature is invalid, see also
    /// [`Self::recover_signer`].
    pub fn recover_signers<'a, T>(txes: T, num_txes: usize) -> Option<Vec<Address>>
    where
        T: IntoParallelIterator<Item = &'a Self> + IntoIterator<Item = &'a Self> + Send,
    {
        if num_txes < *PARALLEL_SENDER_RECOVERY_THRESHOLD {
            txes.into_iter().map(|tx| tx.recover_signer()).collect()
        } else {
            txes.into_par_iter().map(|tx| tx.recover_signer()).collect()
        }
    }

    /// Recovers a list of signers from a transaction list iterator _without ensuring that the
    /// signature has a low `s` value_.
    ///
    /// Returns `None`, if some transaction's signature is invalid, see also
    /// [`Self::recover_signer_unchecked`].
    pub fn recover_signers_unchecked<'a, T>(txes: T, num_txes: usize) -> Option<Vec<Address>>
    where
        T: IntoParallelIterator<Item = &'a Self> + IntoIterator<Item = &'a Self>,
    {
        if num_txes < *PARALLEL_SENDER_RECOVERY_THRESHOLD {
            txes.into_iter().map(|tx| tx.recover_signer_unchecked()).collect()
        } else {
            txes.into_par_iter().map(|tx| tx.recover_signer_unchecked()).collect()
        }
    }

    /// Returns the [`TransactionSignedEcRecovered`] transaction with the given sender.
    #[inline]
    pub const fn with_signer(self, signer: Address) -> TransactionSignedEcRecovered {
        TransactionSignedEcRecovered::from_signed_transaction(self, signer)
    }

    /// Consumes the type, recover signer and return [`TransactionSignedEcRecovered`]
    ///
    /// Returns `None` if the transaction's signature is invalid, see also [`Self::recover_signer`].
    pub fn into_ecrecovered(self) -> Option<TransactionSignedEcRecovered> {
        let signer = self.recover_signer()?;
        Some(TransactionSignedEcRecovered { signed_transaction: self, signer })
    }

    /// Consumes the type, recover signer and return [`TransactionSignedEcRecovered`] _without
    /// ensuring that the signature has a low `s` value_ (EIP-2).
    ///
    /// Returns `None` if the transaction's signature is invalid, see also
    /// [`Self::recover_signer_unchecked`].
    pub fn into_ecrecovered_unchecked(self) -> Option<TransactionSignedEcRecovered> {
        let signer = self.recover_signer_unchecked()?;
        Some(TransactionSignedEcRecovered { signed_transaction: self, signer })
    }

    /// Tries to recover signer and return [`TransactionSignedEcRecovered`] by cloning the type.
    pub fn try_ecrecovered(&self) -> Option<TransactionSignedEcRecovered> {
        let signer = self.recover_signer()?;
        Some(TransactionSignedEcRecovered { signed_transaction: self.clone(), signer })
    }

    /// Tries to recover signer and return [`TransactionSignedEcRecovered`].
    ///
    /// Returns `Err(Self)` if the transaction's signature is invalid, see also
    /// [`Self::recover_signer`].
    pub fn try_into_ecrecovered(self) -> Result<TransactionSignedEcRecovered, Self> {
        match self.recover_signer() {
            None => Err(self),
            Some(signer) => Ok(TransactionSignedEcRecovered { signed_transaction: self, signer }),
        }
    }

    /// Tries to recover signer and return [`TransactionSignedEcRecovered`]. _without ensuring that
    /// the signature has a low `s` value_ (EIP-2).
    ///
    /// Returns `Err(Self)` if the transaction's signature is invalid, see also
    /// [`Self::recover_signer_unchecked`].
    pub fn try_into_ecrecovered_unchecked(self) -> Result<TransactionSignedEcRecovered, Self> {
        match self.recover_signer_unchecked() {
            None => Err(self),
            Some(signer) => Ok(TransactionSignedEcRecovered { signed_transaction: self, signer }),
        }
    }

    /// Calculate transaction hash, eip2728 transaction does not contain rlp header and start with
    /// tx type.
    pub fn recalculate_hash(&self) -> B256 {
        keccak256(self.encoded_2718())
    }

    /// Create a new signed transaction from a transaction and its signature.
    ///
    /// This will also calculate the transaction hash using its encoding.
    pub fn from_transaction_and_signature(transaction: Transaction, signature: Signature) -> Self {
        let mut initial_tx = Self { transaction, hash: Default::default(), signature };
        initial_tx.hash = initial_tx.recalculate_hash();
        initial_tx
    }

    /// Calculate a heuristic for the in-memory size of the [`TransactionSigned`].
    #[inline]
    pub fn size(&self) -> usize {
        mem::size_of::<TxHash>() + self.transaction.size() + mem::size_of::<Signature>()
    }

    /// Decodes legacy transaction from the data buffer into a tuple.
    ///
    /// This expects `rlp(legacy_tx)`
    ///
    /// Refer to the docs for [`Self::decode_rlp_legacy_transaction`] for details on the exact
    /// format expected.
    pub(crate) fn decode_rlp_legacy_transaction_tuple(
        data: &mut &[u8],
    ) -> alloy_rlp::Result<(TxLegacy, TxHash, Signature)> {
        let original_encoding = *data;

        let header = alloy_rlp::Header::decode(data)?;
        let remaining_len = data.len();

        let transaction_payload_len = header.payload_length;

        if transaction_payload_len > remaining_len {
            return Err(RlpError::InputTooShort);
        }

        let mut transaction = TxLegacy {
            nonce: Decodable::decode(data)?,
            gas_price: Decodable::decode(data)?,
            gas_limit: Decodable::decode(data)?,
            to: Decodable::decode(data)?,
            value: Decodable::decode(data)?,
            input: Decodable::decode(data)?,
            chain_id: None,
        };

        let v: u64 = Decodable::decode(data)?;
        let r: U256 = Decodable::decode(data)?;
        let s: U256 = Decodable::decode(data)?;

        let tx_length = header.payload_length + header.length();
        let hash = keccak256(&original_encoding[..tx_length]);

        // Handle both pre-bedrock and regular cases
        let (signature, chain_id) = if v == 0 && r.is_zero() && s.is_zero() {
            // Pre-bedrock system transactions case
            (Signature::new(r, s, false), None)
        } else {
            // Regular transaction case
            let (parity, chain_id) = from_eip155_value(v)
                .ok_or(alloy_rlp::Error::Custom("invalid parity for legacy transaction"))?;
            (Signature::new(r, s, parity), chain_id)
        };

        // Set chain ID and verify length
        transaction.chain_id = chain_id;
        let decoded = remaining_len - data.len();
        if decoded != transaction_payload_len {
            return Err(RlpError::UnexpectedLength);
        }

        Ok((transaction, hash, signature))
    }

    /// Decodes legacy transaction from the data buffer.
    ///
    /// This should be used _only_ be used in general transaction decoding methods, which have
    /// already ensured that the input is a legacy transaction with the following format:
    /// `rlp(legacy_tx)`
    ///
    /// Legacy transactions are encoded as lists, so the input should start with a RLP list header.
    ///
    /// This expects `rlp(legacy_tx)`
    // TODO: make buf advancement semantics consistent with `decode_enveloped_typed_transaction`,
    // so decoding methods do not need to manually advance the buffer
    pub fn decode_rlp_legacy_transaction(data: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let (transaction, hash, signature) = Self::decode_rlp_legacy_transaction_tuple(data)?;
        let signed = Self { transaction: Transaction::Legacy(transaction), hash, signature };
        Ok(signed)
    }
}

impl alloy_consensus::Transaction for TransactionSigned {
    fn chain_id(&self) -> Option<ChainId> {
        self.deref().chain_id()
    }

    fn nonce(&self) -> u64 {
        self.deref().nonce()
    }

    fn gas_limit(&self) -> u64 {
        self.deref().gas_limit()
    }

    fn gas_price(&self) -> Option<u128> {
        self.deref().gas_price()
    }

    fn max_fee_per_gas(&self) -> u128 {
        self.deref().max_fee_per_gas()
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        self.deref().max_priority_fee_per_gas()
    }

    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        self.deref().max_fee_per_blob_gas()
    }

    fn priority_fee_or_price(&self) -> u128 {
        self.deref().priority_fee_or_price()
    }

    fn value(&self) -> U256 {
        self.deref().value()
    }

    fn input(&self) -> &Bytes {
        self.deref().input()
    }

    fn ty(&self) -> u8 {
        self.deref().ty()
    }

    fn access_list(&self) -> Option<&AccessList> {
        self.deref().access_list()
    }

    fn blob_versioned_hashes(&self) -> Option<&[B256]> {
        alloy_consensus::Transaction::blob_versioned_hashes(self.deref())
    }

    fn authorization_list(&self) -> Option<&[SignedAuthorization]> {
        self.deref().authorization_list()
    }

    fn kind(&self) -> TxKind {
        self.deref().kind()
    }
}

impl From<TransactionSignedEcRecovered> for TransactionSigned {
    fn from(recovered: TransactionSignedEcRecovered) -> Self {
        recovered.signed_transaction
    }
}

impl Encodable for TransactionSigned {
    /// This encodes the transaction _with_ the signature, and an rlp header.
    ///
    /// For legacy transactions, it encodes the transaction data:
    /// `rlp(tx-data)`
    ///
    /// For EIP-2718 typed transactions, it encodes the transaction type followed by the rlp of the
    /// transaction:
    /// `rlp(tx-type || rlp(tx-data))`
    fn encode(&self, out: &mut dyn BufMut) {
        self.network_encode(out);
    }

    fn length(&self) -> usize {
        let mut payload_length = self.encode_2718_len();
        if !self.is_legacy() {
            payload_length += alloy_rlp::Header { list: false, payload_length }.length();
        }

        payload_length
    }
}

impl Decodable for TransactionSigned {
    /// This `Decodable` implementation only supports decoding rlp encoded transactions as it's used
    /// by p2p.
    ///
    /// The p2p encoding format always includes an RLP header, although the type RLP header depends
    /// on whether or not the transaction is a legacy transaction.
    ///
    /// If the transaction is a legacy transaction, it is just encoded as a RLP list:
    /// `rlp(tx-data)`.
    ///
    /// If the transaction is a typed transaction, it is encoded as a RLP string:
    /// `rlp(tx-type || rlp(tx-data))`
    ///
    /// This can be used for decoding all signed transactions in p2p `BlockBodies` responses.
    ///
    /// This cannot be used for decoding EIP-4844 transactions in p2p `PooledTransactions`, since
    /// the EIP-4844 variant of [`TransactionSigned`] does not include the blob sidecar.
    ///
    /// For a method suitable for decoding pooled transactions, see \[`PooledTransactionsElement`\].
    ///
    /// CAUTION: Due to a quirk in [`Header::decode`], this method will succeed even if a typed
    /// transaction is encoded in this format, and does not start with a RLP header:
    /// `tx-type || rlp(tx-data)`.
    ///
    /// This is because [`Header::decode`] does not advance the buffer, and returns a length-1
    /// string header if the first byte is less than `0xf7`.
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Self::network_decode(buf).map_err(Into::into)
    }
}

impl Encodable2718 for TransactionSigned {
    fn type_flag(&self) -> Option<u8> {
        match self.transaction.tx_type() {
            TxType::Legacy => None,
            tx_type => Some(tx_type as u8),
        }
    }

    fn encode_2718_len(&self) -> usize {
        match &self.transaction {
            Transaction::Legacy(legacy_tx) => legacy_tx.eip2718_encoded_length(&self.signature),
            Transaction::Eip2930(access_list_tx) => {
                access_list_tx.eip2718_encoded_length(&self.signature)
            }
            Transaction::Eip1559(dynamic_fee_tx) => {
                dynamic_fee_tx.eip2718_encoded_length(&self.signature)
            }
            Transaction::Eip4844(blob_tx) => blob_tx.eip2718_encoded_length(&self.signature),
            Transaction::Eip7702(set_code_tx) => {
                set_code_tx.eip2718_encoded_length(&self.signature)
            }
            Transaction::Deposit(deposit_tx) => deposit_tx.eip2718_encoded_length(),
        }
    }

    fn encode_2718(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.transaction.eip2718_encode(&self.signature, out)
    }
}

impl Decodable2718 for TransactionSigned {
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Eip2718Result<Self> {
        match ty.try_into().map_err(|_| Eip2718Error::UnexpectedType(ty))? {
            TxType::Legacy => Err(Eip2718Error::UnexpectedType(0)),
            TxType::Eip2930 => {
                let (tx, signature, hash) = TxEip2930::rlp_decode_signed(buf)?.into_parts();
                Ok(Self { transaction: Transaction::Eip2930(tx), signature, hash })
            }
            TxType::Eip1559 => {
                let (tx, signature, hash) = TxEip1559::rlp_decode_signed(buf)?.into_parts();
                Ok(Self { transaction: Transaction::Eip1559(tx), signature, hash })
            }
            TxType::Eip7702 => {
                let (tx, signature, hash) = TxEip7702::rlp_decode_signed(buf)?.into_parts();
                Ok(Self { transaction: Transaction::Eip7702(tx), signature, hash })
            }
            TxType::Eip4844 => {
                let (tx, signature, hash) = TxEip4844::rlp_decode_signed(buf)?.into_parts();
                Ok(Self { transaction: Transaction::Eip4844(tx), signature, hash })
            }
            TxType::Deposit => Ok(Self::from_transaction_and_signature(
                Transaction::Deposit(TxDeposit::rlp_decode(buf)?),
                TxDeposit::signature(),
            )),
        }
    }

    fn fallback_decode(buf: &mut &[u8]) -> Eip2718Result<Self> {
        Ok(Self::decode_rlp_legacy_transaction(buf)?)
    }
}

/// Signed transaction with recovered signer.
#[derive(Debug, Clone, PartialEq, Hash, Eq, AsRef, Deref)]
pub struct TransactionSignedEcRecovered {
    /// Signer of the transaction
    signer: Address,
    /// Signed transaction
    #[deref]
    #[as_ref]
    signed_transaction: TransactionSigned,
}

// === impl TransactionSignedEcRecovered ===

impl TransactionSignedEcRecovered {
    /// Signer of transaction recovered from signature
    pub const fn signer(&self) -> Address {
        self.signer
    }

    /// Returns a reference to [`TransactionSigned`]
    pub const fn as_signed(&self) -> &TransactionSigned {
        &self.signed_transaction
    }

    /// Transform back to [`TransactionSigned`]
    pub fn into_signed(self) -> TransactionSigned {
        self.signed_transaction
    }

    /// Dissolve Self to its component
    pub fn to_components(self) -> (TransactionSigned, Address) {
        (self.signed_transaction, self.signer)
    }

    /// Create [`TransactionSignedEcRecovered`] from [`TransactionSigned`] and [`Address`] of the
    /// signer.
    #[inline]
    pub const fn from_signed_transaction(
        signed_transaction: TransactionSigned,
        signer: Address,
    ) -> Self {
        Self { signed_transaction, signer }
    }
}

impl Encodable for TransactionSignedEcRecovered {
    /// This encodes the transaction _with_ the signature, and an rlp header.
    ///
    /// Refer to docs for [`TransactionSigned::encode`] for details on the exact format.
    fn encode(&self, out: &mut dyn BufMut) {
        self.signed_transaction.encode(out)
    }

    fn length(&self) -> usize {
        self.signed_transaction.length()
    }
}

impl Decodable for TransactionSignedEcRecovered {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let signed_transaction = TransactionSigned::decode(buf)?;
        let signer = match &signed_transaction.transaction {
            // For deposit transactions, use from address
            Transaction::Deposit(TxDeposit { from, .. }) => *from,

            // For system transactions with empty signatures
            tx if tx.is_legacy() && signed_transaction.signature == TxDeposit::signature() => {
                Address::ZERO
            }

            // Normal signature recovery for other type of transactions
            _ => signed_transaction
                .recover_signer()
                .ok_or(RlpError::Custom("Unable to recover decoded transaction signer."))?,
        };
        Ok(Self { signer, signed_transaction })
    }
}

#[cfg(test)]
mod tests {
    use crate::ovm_file_codec::{
        Block, BlockBody, OvmBlockFileCodec, TransactionSigned, TransactionSignedEcRecovered,
    };
    use alloy_consensus::{Header, TxLegacy};
    use alloy_primitives::{
        address, hex, Address, PrimitiveSignature as Signature, TxKind, B256, U256,
    };
    use reth_primitives::transaction::Transaction;
    const DEPOSIT_FUNCTION_SELECTOR: [u8; 4] = [0xb6, 0xb5, 0x5f, 0x25];
    use alloy_rlp::{BytesMut, Decodable, Encodable};
    use op_alloy_consensus::TxDeposit;
    use tokio_util::codec::{Decoder, Encoder};

    #[test]
    fn test_decode_legacy_transactions() {
        // Test Case 1: contract deposit (regular L2 transaction calling deposit() function)
        // tx: https://optimistic.etherscan.io/getRawTx?tx=0x7860252963a2df21113344f323035ef59648638a571eef742e33d789602c7a1c
        let deposit_tx_bytes = hex!("f88881f0830f481c830c6e4594a75127121d28a9bf848f3b70e7eea26570aa770080a4b6b55f2500000000000000000000000000000000000000000000000000000000000710b238a0d5c622d92ddf37f9c18a3465a572f74d8b1aeaf50c1cfb10b3833242781fd45fa02c4f1d5819bf8b70bf651e7a063b7db63c55bd336799c6ae3e5bc72ad6ef3def");
        let deposit_decoded = TransactionSigned::decode(&mut &deposit_tx_bytes[..]).unwrap();

        // Verify deposit transaction
        let deposit_tx = match &deposit_decoded.transaction {
            Transaction::Legacy(ref tx) => tx,
            _ => panic!("Expected legacy transaction for NFT deposit"),
        };

        assert_eq!(
            deposit_tx.to,
            TxKind::Call(address!("a75127121d28a9bf848f3b70e7eea26570aa7700"))
        );
        assert_eq!(deposit_tx.nonce, 240);
        assert_eq!(deposit_tx.gas_price, 1001500);
        assert_eq!(deposit_tx.gas_limit, 814661);
        assert_eq!(deposit_tx.value, U256::ZERO);
        assert_eq!(&deposit_tx.input.as_ref()[0..4], DEPOSIT_FUNCTION_SELECTOR);
        assert_eq!(deposit_tx.chain_id, Some(10));
        assert_eq!(
            deposit_decoded.signature.r(),
            U256::from_str_radix(
                "d5c622d92ddf37f9c18a3465a572f74d8b1aeaf50c1cfb10b3833242781fd45f",
                16
            )
            .unwrap()
        );
        assert_eq!(
            deposit_decoded.signature.s(),
            U256::from_str_radix(
                "2c4f1d5819bf8b70bf651e7a063b7db63c55bd336799c6ae3e5bc72ad6ef3def",
                16
            )
            .unwrap()
        );

        // Test Case 2: pre-bedrock system transaction from block 105235052
        // tx: https://optimistic.etherscan.io/getRawTx?tx=0xe20b11349681dd049f8df32f5cdbb4c68d46b537685defcd86c7fa42cfe75b9e
        let system_tx_bytes = hex!("f9026c830d899383124f808302a77e94a0cc33dd6f4819d473226257792afe230ec3c67f80b902046c459a280000000000000000000000004d73adb72bc3dd368966edd0f0b2148401a178e2000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000647fac7f00000000000000000000000000000000000000000000000000000000000001400000000000000000000000000000000000000000000000000000000000000084704316e5000000000000000000000000000000000000000000000000000000000000006e10975631049de3c008989b0d8c19fc720dc556ca01abfbd794c6eb5075dd000d000000000000000000000000000000000000000000000000000000000000001410975631049de3c008989b0d8c19fc720dc556ca01abfbd794c6eb5075dd000d000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000082a39325251d44e11f3b6d92f9382438eb6c8b5068d4a488d4f177b26f2ca20db34ae53467322852afcc779f25eafd124c5586f54b9026497ba934403d4c578e3c1b5aa754c918ee2ecd25402df656c2419717e4017a7aecb84af3914fd3c7bf6930369c4e6ff76950246b98e354821775f02d33cdbee5ef6aed06c15b75691692d31c00000000000000000000000000000000000000000000000000000000000038a0e8991e95e66d809f4b6fb0af27c31368ca0f30e657165c428aa681ec5ea25bbea013ed325bd97365087ec713e9817d252b59113ea18430b71a5890c4eeb6b9efc4");
        let system_decoded = TransactionSigned::decode(&mut &system_tx_bytes[..]).unwrap();

        // Verify system transaction
        assert!(system_decoded.is_legacy());

        let system_tx = match &system_decoded.transaction {
            Transaction::Legacy(ref tx) => tx,
            _ => panic!("Expected Legacy transaction"),
        };

        assert_eq!(system_tx.nonce, 887187);
        assert_eq!(system_tx.gas_price, 1200000);
        assert_eq!(system_tx.gas_limit, 173950);
        assert_eq!(
            system_tx.to,
            TxKind::Call(address!("a0cc33dd6f4819d473226257792afe230ec3c67f"))
        );
        assert_eq!(system_tx.value, U256::ZERO);
        assert_eq!(system_tx.chain_id, Some(10));

        assert_eq!(
            system_decoded.signature.r(),
            U256::from_str_radix(
                "e8991e95e66d809f4b6fb0af27c31368ca0f30e657165c428aa681ec5ea25bbe",
                16
            )
            .unwrap()
        );
        assert_eq!(
            system_decoded.signature.s(),
            U256::from_str_radix(
                "13ed325bd97365087ec713e9817d252b59113ea18430b71a5890c4eeb6b9efc4",
                16
            )
            .unwrap()
        );
        assert_eq!(
            system_decoded.hash,
            B256::from(hex!("e20b11349681dd049f8df32f5cdbb4c68d46b537685defcd86c7fa42cfe75b9e"))
        );

        // Verify RLP encoding/decoding roundtrips
        let mut encoded = Vec::new();
        system_decoded.encode(&mut encoded);
        assert_eq!(&encoded[..], &system_tx_bytes[..]);
    }

    #[test]
    fn test_decode_signed_ec_recovered_transaction() {
        // tx https://optimistic.etherscan.io/getRawTx?tx=0x7860252963a2df21113344f323035ef59648638a571eef742e33d789602c7a1c
        // This is a properly RLP encoded transaction that includes signature fields
        let tx_bytes = hex!("f88e830310f1840479e442830f424094eb4378aacdfb7d02a0c9accfac2e90ad545ad62a80a70000053800ac70a8221300000000000049e7c4ebf673a7b663033bf6debe00000000003e97520d38a08f9b10c30337089472b821c408dff35ab37e2d52c4fbf3913f64066009981963a04b5ea18e18b3cc7f808d0e25ea1e138bb5ed193d5b2b24441a77978e3b5f385d");
        let tx = TransactionSigned::decode(&mut &tx_bytes[..]).unwrap();
        let recovered = tx.into_ecrecovered().unwrap();

        let decoded =
            TransactionSignedEcRecovered::decode(&mut &alloy_rlp::encode(&recovered)[..]).unwrap();
        assert_eq!(recovered, decoded)
    }

    #[test]
    fn test_recover_legacy_singer() {
        // tx https://optimistic.etherscan.io/getRawTx?tx=0x7860252963a2df21113344f323035ef59648638a571eef742e33d789602c7a1c
        let tx_bytes = hex!("f88881f0830f481c830c6e4594a75127121d28a9bf848f3b70e7eea26570aa770080a4b6b55f2500000000000000000000000000000000000000000000000000000000000710b238a0d5c622d92ddf37f9c18a3465a572f74d8b1aeaf50c1cfb10b3833242781fd45fa02c4f1d5819bf8b70bf651e7a063b7db63c55bd336799c6ae3e5bc72ad6ef3def");
        let tx =
            TransactionSigned::decode_rlp_legacy_transaction(&mut tx_bytes.as_slice()).unwrap();
        assert!(tx.is_legacy());
        let sender = tx.recover_signer().unwrap();
        println!("Sender: {:?}", sender);
        assert_eq!(sender, address!("6d74af94c8e72805ba3f7ce357a5b12ecb3ad71a"));
    }

    #[test]
    fn test_deposit_tx_roundtrip() {
        let mut codec = OvmBlockFileCodec;
        let mut buffer = BytesMut::new();

        // Create a block with a deposit transaction
        let deposit_tx = Transaction::Deposit(TxDeposit {
            source_hash: B256::random(),
            from: Address::random(),
            to: TxKind::Call(Address::random()),
            mint: Some(1000000000000000000u128),
            value: U256::from(1000000000000000000u128),
            gas_limit: 100000,
            is_system_transaction: false,
            input: Default::default(),
        });

        let signed_tx = TransactionSigned::from_transaction_and_signature(
            deposit_tx.clone(),
            TxDeposit::signature(),
        );

        let block = Block {
            header: Header::default(),
            body: BlockBody { transactions: vec![signed_tx], ommers: vec![], withdrawals: None },
        };

        codec.encode(block.clone(), &mut buffer).unwrap();
        let decoded = codec.decode(&mut buffer).unwrap().unwrap();

        assert_eq!(decoded, block);
        assert_eq!(decoded.body.transactions.len(), 1);
        assert!(
            matches!(decoded.body.transactions[0].transaction, Transaction::Deposit(_)),
            "Expected Deposit transaction"
        );
    }

    #[test]
    fn test_decode_encode_block_with_multiple_tx_types() {
        let mut codec = OvmBlockFileCodec;
        let mut buffer = BytesMut::new();

        // create transactions of different types
        let legacy_tx = TransactionSigned::from_transaction_and_signature(
            Transaction::Legacy(TxLegacy {
                chain_id: Some(10),
                nonce: 1,
                gas_price: 1000000000,
                gas_limit: 100000,
                to: Address::random().into(),
                value: U256::from(1000000000000000u64),
                input: Default::default(),
            }),
            Signature::test_signature(),
        );

        let deposit_tx = TransactionSigned::from_transaction_and_signature(
            Transaction::Deposit(TxDeposit {
                source_hash: B256::random(),
                from: Address::random(),
                to: TxKind::Call(Address::random()),
                mint: Some(1000000000000000000u128),
                value: U256::from(1000000000000000000u128),
                gas_limit: 100000,
                is_system_transaction: false,
                input: Default::default(),
            }),
            TxDeposit::signature(),
        );

        let block = Block {
            header: Header::default(),
            body: BlockBody {
                transactions: vec![legacy_tx, deposit_tx],
                ommers: vec![],
                withdrawals: None,
            },
        };

        codec.encode(block.clone(), &mut buffer).unwrap();
        let decoded = codec.decode(&mut buffer).unwrap().unwrap();

        assert_eq!(decoded, block);
        assert_eq!(decoded.body.transactions.len(), 2);
        assert!(matches!(decoded.body.transactions[0].transaction, Transaction::Legacy(_)));
        assert!(matches!(decoded.body.transactions[1].transaction, Transaction::Deposit(_)));
    }
}
