//! Block-related consensus types.

mod header;
pub use header::{BlockHeader, Header};

mod traits;
pub use traits::EthBlock;

mod meta;
pub use meta::{HeaderInfo, HeaderRoots};

#[cfg(all(feature = "serde", feature = "serde-bincode-compat"))]
pub(crate) use header::serde_bincode_compat;

use crate::Transaction;
use alloc::vec::Vec;
use alloy_eips::{eip2718::WithEncoded, eip4895::Withdrawals, Encodable2718, Typed2718};
use alloy_primitives::{keccak256, Sealable, Sealed, B256};
use alloy_rlp::{Decodable, Encodable, RlpDecodable, RlpEncodable};

/// Ethereum full block.
///
/// Withdrawals can be optionally included at the end of the RLP encoded message.
///
/// Taken from [reth-primitives](https://github.com/paradigmxyz/reth)
///
/// See p2p block encoding reference: <https://github.com/ethereum/devp2p/blob/master/caps/eth.md#block-encoding-and-validity>
#[derive(Debug, Clone, PartialEq, Eq, derive_more::Deref)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "borsh", derive(borsh::BorshSerialize, borsh::BorshDeserialize))]
pub struct Block<T, H = Header> {
    /// Block header.
    #[deref]
    pub header: H,
    /// Block body.
    pub body: BlockBody<T, H>,
}

impl<T, H> Block<T, H> {
    /// Creates a new block with the given header and body.
    pub const fn new(header: H, body: BlockBody<T, H>) -> Self {
        Self { header, body }
    }

    /// Creates a new empty uncle block.
    pub fn uncle(header: H) -> Self {
        Self { header, body: Default::default() }
    }

    /// Consumes the block and returns the header.
    pub fn into_header(self) -> H {
        self.header
    }

    /// Consumes the block and returns the body.
    pub fn into_body(self) -> BlockBody<T, H> {
        self.body
    }

    /// Converts the block's header type by applying a function to it.
    pub fn map_header<U>(self, mut f: impl FnMut(H) -> U) -> Block<T, U> {
        Block { header: f(self.header), body: self.body.map_ommers(f) }
    }

    /// Converts the block's header type by applying a fallible function to it.
    pub fn try_map_header<U, E>(
        self,
        mut f: impl FnMut(H) -> Result<U, E>,
    ) -> Result<Block<T, U>, E> {
        Ok(Block { header: f(self.header)?, body: self.body.try_map_ommers(f)? })
    }

    /// Converts the block's transaction type to the given alternative that is `From<T>`
    pub fn convert_transactions<U>(self) -> Block<U, H>
    where
        U: From<T>,
    {
        self.map_transactions(U::from)
    }

    /// Converts the block's transaction to the given alternative that is `TryFrom<T>`
    ///
    /// Returns the block with the new transaction type if all conversions were successful.
    pub fn try_convert_transactions<U>(self) -> Result<Block<U, H>, U::Error>
    where
        U: TryFrom<T>,
    {
        self.try_map_transactions(U::try_from)
    }

    /// Converts the block's transaction type by applying a function to each transaction.
    ///
    /// Returns the block with the new transaction type.
    pub fn map_transactions<U>(self, f: impl FnMut(T) -> U) -> Block<U, H> {
        Block {
            header: self.header,
            body: BlockBody {
                transactions: self.body.transactions.into_iter().map(f).collect(),
                ommers: self.body.ommers,
                withdrawals: self.body.withdrawals,
            },
        }
    }

    /// Converts the block's transaction type by applying a fallible function to each transaction.
    ///
    /// Returns the block with the new transaction type if all transactions were successfully.
    pub fn try_map_transactions<U, E>(
        self,
        f: impl FnMut(T) -> Result<U, E>,
    ) -> Result<Block<U, H>, E> {
        Ok(Block {
            header: self.header,
            body: BlockBody {
                transactions: self
                    .body
                    .transactions
                    .into_iter()
                    .map(f)
                    .collect::<Result<_, _>>()?,
                ommers: self.body.ommers,
                withdrawals: self.body.withdrawals,
            },
        })
    }

    /// Converts the transactions in the block's body to `WithEncoded<T>` by encoding them via
    /// [`Encodable2718`]
    pub fn into_with_encoded2718(self) -> Block<WithEncoded<T>, H>
    where
        T: Encodable2718,
    {
        self.map_transactions(|tx| tx.into_encoded())
    }

    /// Replaces the header of the block.
    ///
    /// Note: This method only replaces the main block header. If you need to transform
    /// the ommer headers as well, use [`map_header`](Self::map_header) instead.
    pub fn with_header(mut self, header: H) -> Self {
        self.header = header;
        self
    }

    /// Encodes the [`Block`] given header and block body.
    ///
    /// Returns the rlp encoded block.
    ///
    /// This is equivalent to `block.encode`.
    pub fn rlp_encoded_from_parts(header: &H, body: &BlockBody<T, H>) -> Vec<u8>
    where
        H: Encodable,
        T: Encodable,
    {
        let helper = block_rlp::HelperRef::from_parts(header, body);
        let mut buf = Vec::with_capacity(helper.length());
        helper.encode(&mut buf);
        buf
    }

    /// Encodes the [`Block`] given header and block body
    ///
    /// This is equivalent to `block.encode`.
    pub fn rlp_encode_from_parts(
        header: &H,
        body: &BlockBody<T, H>,
        out: &mut dyn alloy_rlp::bytes::BufMut,
    ) where
        H: Encodable,
        T: Encodable,
    {
        block_rlp::HelperRef::from_parts(header, body).encode(out)
    }

    /// Returns the RLP encoded length of the block's header and body.
    pub fn rlp_length_for(header: &H, body: &BlockBody<T, H>) -> usize
    where
        H: Encodable,
        T: Encodable,
    {
        block_rlp::HelperRef::from_parts(header, body).length()
    }
}

impl<T, H> Default for Block<T, H>
where
    H: Default,
{
    fn default() -> Self {
        Self { header: Default::default(), body: Default::default() }
    }
}

impl<T, H> From<Block<T, H>> for BlockBody<T, H> {
    fn from(block: Block<T, H>) -> Self {
        block.into_body()
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a, T, H> arbitrary::Arbitrary<'a> for Block<T, H>
where
    T: arbitrary::Arbitrary<'a>,
    H: arbitrary::Arbitrary<'a>,
{
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        Ok(Self { header: u.arbitrary()?, body: u.arbitrary()? })
    }
}

/// A response to `GetBlockBodies`, containing bodies if any bodies were found.
///
/// Withdrawals can be optionally included at the end of the RLP encoded message.
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "borsh", derive(borsh::BorshSerialize, borsh::BorshDeserialize))]
#[rlp(trailing)]
pub struct BlockBody<T, H = Header> {
    /// Transactions in this block.
    pub transactions: Vec<T>,
    /// Ommers/uncles header.
    pub ommers: Vec<H>,
    /// Block withdrawals.
    pub withdrawals: Option<Withdrawals>,
}

impl<T, H> Default for BlockBody<T, H> {
    fn default() -> Self {
        Self { transactions: Vec::new(), ommers: Vec::new(), withdrawals: None }
    }
}

impl<T, H> BlockBody<T, H> {
    /// Returns an iterator over all transactions.
    #[inline]
    pub fn transactions(&self) -> impl Iterator<Item = &T> + '_ {
        self.transactions.iter()
    }

    /// Create a [`Block`] from the body and its header.
    pub const fn into_block(self, header: H) -> Block<T, H> {
        Block { header, body: self }
    }

    /// Calculate the ommers root for the block body.
    pub fn calculate_ommers_root(&self) -> B256
    where
        H: Encodable,
    {
        crate::proofs::calculate_ommers_root(&self.ommers)
    }

    /// Returns an iterator over the hashes of the ommers in the block body.
    pub fn ommers_hashes(&self) -> impl Iterator<Item = B256> + '_
    where
        H: Sealable,
    {
        self.ommers.iter().map(|h| h.hash_slow())
    }

    /// Calculate the withdrawals root for the block body, if withdrawals exist. If there are no
    /// withdrawals, this will return `None`.
    pub fn calculate_withdrawals_root(&self) -> Option<B256> {
        self.withdrawals.as_ref().map(|w| crate::proofs::calculate_withdrawals_root(w))
    }

    /// Converts the body's ommers type by applying a function to it.
    pub fn map_ommers<U>(self, f: impl FnMut(H) -> U) -> BlockBody<T, U> {
        BlockBody {
            transactions: self.transactions,
            ommers: self.ommers.into_iter().map(f).collect(),
            withdrawals: self.withdrawals,
        }
    }

    /// Converts the body's ommers type by applying a fallible function to it.
    pub fn try_map_ommers<U, E>(
        self,
        f: impl FnMut(H) -> Result<U, E>,
    ) -> Result<BlockBody<T, U>, E> {
        Ok(BlockBody {
            transactions: self.transactions,
            ommers: self.ommers.into_iter().map(f).collect::<Result<Vec<_>, _>>()?,
            withdrawals: self.withdrawals,
        })
    }
}

impl<T: Transaction, H> BlockBody<T, H> {
    /// Returns an iterator over all blob versioned hashes from the block body.
    #[inline]
    pub fn blob_versioned_hashes_iter(&self) -> impl Iterator<Item = &B256> + '_ {
        self.eip4844_transactions_iter().filter_map(|tx| tx.blob_versioned_hashes()).flatten()
    }
}

impl<T: Typed2718, H> BlockBody<T, H> {
    /// Returns whether or not the block body contains any blob transactions.
    #[inline]
    pub fn has_eip4844_transactions(&self) -> bool {
        self.transactions.iter().any(|tx| tx.is_eip4844())
    }

    /// Returns whether or not the block body contains any EIP-7702 transactions.
    #[inline]
    pub fn has_eip7702_transactions(&self) -> bool {
        self.transactions.iter().any(|tx| tx.is_eip7702())
    }

    /// Returns an iterator over all blob transactions of the block.
    #[inline]
    pub fn eip4844_transactions_iter(&self) -> impl Iterator<Item = &T> + '_ {
        self.transactions.iter().filter(|tx| tx.is_eip4844())
    }
}

/// We need to implement RLP traits manually because we currently don't have a way to flatten
/// [`BlockBody`] into [`Block`].
mod block_rlp {
    use super::*;

    #[derive(RlpDecodable)]
    #[rlp(trailing)]
    struct Helper<T, H> {
        header: H,
        transactions: Vec<T>,
        ommers: Vec<H>,
        withdrawals: Option<Withdrawals>,
    }

    #[derive(RlpEncodable)]
    #[rlp(trailing)]
    pub(crate) struct HelperRef<'a, T, H> {
        pub(crate) header: &'a H,
        pub(crate) transactions: &'a Vec<T>,
        pub(crate) ommers: &'a Vec<H>,
        pub(crate) withdrawals: Option<&'a Withdrawals>,
    }

    impl<'a, T, H> HelperRef<'a, T, H> {
        pub(crate) const fn from_parts(header: &'a H, body: &'a BlockBody<T, H>) -> Self {
            Self {
                header,
                transactions: &body.transactions,
                ommers: &body.ommers,
                withdrawals: body.withdrawals.as_ref(),
            }
        }
    }

    impl<'a, T, H> From<&'a Block<T, H>> for HelperRef<'a, T, H> {
        fn from(block: &'a Block<T, H>) -> Self {
            let Block { header, body: BlockBody { transactions, ommers, withdrawals } } = block;
            Self { header, transactions, ommers, withdrawals: withdrawals.as_ref() }
        }
    }

    impl<T: Encodable, H: Encodable> Encodable for Block<T, H> {
        fn encode(&self, out: &mut dyn alloy_rlp::bytes::BufMut) {
            let helper: HelperRef<'_, T, H> = self.into();
            helper.encode(out)
        }

        fn length(&self) -> usize {
            let helper: HelperRef<'_, T, H> = self.into();
            helper.length()
        }
    }

    impl<T: Decodable, H: Decodable> Decodable for Block<T, H> {
        fn decode(b: &mut &[u8]) -> alloy_rlp::Result<Self> {
            let Helper { header, transactions, ommers, withdrawals } = Helper::decode(b)?;
            Ok(Self { header, body: BlockBody { transactions, ommers, withdrawals } })
        }
    }

    impl<T: Decodable, H: Decodable> Block<T, H> {
        /// Decodes the block from RLP, computing the header hash directly from the RLP bytes.
        ///
        /// This is more efficient than decoding the block and then sealing it, as the header
        /// hash is computed from the raw RLP bytes without re-encoding.
        pub fn decode_sealed(buf: &mut &[u8]) -> alloy_rlp::Result<Sealed<Self>> {
            // Decode the outer block list header
            let block_rlp_head = alloy_rlp::Header::decode(buf)?;
            if !block_rlp_head.list {
                return Err(alloy_rlp::Error::UnexpectedString);
            }

            // Decode header and compute hash from raw RLP bytes
            let header_start = *buf;
            let header = H::decode(buf)?;
            let header_hash = keccak256(&header_start[..header_start.len() - buf.len()]);

            // Decode remaining body fields
            let transactions = Vec::<T>::decode(buf)?;
            let ommers = Vec::<H>::decode(buf)?;
            let withdrawals = if buf.is_empty() { None } else { Some(Decodable::decode(buf)?) };

            let block = Self { header, body: BlockBody { transactions, ommers, withdrawals } };

            Ok(Sealed::new_unchecked(block, header_hash))
        }
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a, T, H> arbitrary::Arbitrary<'a> for BlockBody<T, H>
where
    T: arbitrary::Arbitrary<'a>,
    H: arbitrary::Arbitrary<'a>,
{
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        // first generate up to 100 txs
        let transactions = (0..u.int_in_range(0..=100)?)
            .map(|_| T::arbitrary(u))
            .collect::<arbitrary::Result<Vec<_>>>()?;

        // then generate up to 2 ommers
        let ommers = (0..u.int_in_range(0..=1)?)
            .map(|_| H::arbitrary(u))
            .collect::<arbitrary::Result<Vec<_>>>()?;

        Ok(Self { transactions, ommers, withdrawals: u.arbitrary()? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Signed, TxEnvelope, TxLegacy};
    use alloy_rlp::Encodable;

    #[test]
    fn can_convert_block() {
        let block: Block<Signed<TxLegacy>> = Block::default();
        let _: Block<TxEnvelope> = block.convert_transactions();
    }

    #[test]
    fn decode_sealed_produces_correct_hash() {
        let block: Block<TxEnvelope> = Block::default();
        let expected_hash = block.header.hash_slow();

        let mut encoded = Vec::new();
        block.encode(&mut encoded);

        let mut buf = encoded.as_slice();
        let sealed = Block::<TxEnvelope>::decode_sealed(&mut buf).unwrap();

        assert_eq!(sealed.hash(), expected_hash);
        assert_eq!(*sealed.inner(), block);
    }

    #[test]
    fn header_decode_sealed_produces_correct_hash() {
        let header = Header::default();
        let expected_hash = header.hash_slow();

        let mut encoded = Vec::new();
        header.encode(&mut encoded);

        let mut buf = encoded.as_slice();
        let sealed = Header::decode_sealed(&mut buf).unwrap();

        assert_eq!(sealed.hash(), expected_hash);
        assert_eq!(*sealed.inner(), header);
        assert!(buf.is_empty());
    }

    #[test]
    fn decode_sealed_roundtrip_with_transactions() {
        use crate::{SignableTransaction, TxLegacy};
        use alloy_primitives::{Address, Signature, TxKind, U256};

        let tx = TxLegacy {
            nonce: 1,
            gas_price: 100,
            gas_limit: 21000,
            to: TxKind::Call(Address::ZERO),
            value: U256::from(1000),
            input: Default::default(),
            chain_id: Some(1),
        };
        let sig = Signature::new(U256::from(1), U256::from(2), false);
        let signed = tx.into_signed(sig);
        let envelope: TxEnvelope = signed.into();

        let block = Block {
            header: Header { number: 42, gas_limit: 30_000_000, ..Default::default() },
            body: BlockBody { transactions: vec![envelope], ommers: vec![], withdrawals: None },
        };

        let expected_hash = block.header.hash_slow();

        let mut encoded = Vec::new();
        block.encode(&mut encoded);

        let mut buf = encoded.as_slice();
        let sealed = Block::<TxEnvelope>::decode_sealed(&mut buf).unwrap();

        assert_eq!(sealed.hash(), expected_hash);
        assert_eq!(sealed.header.number, 42);
        assert_eq!(sealed.body.transactions.len(), 1);
        assert!(buf.is_empty());
    }
}

#[cfg(all(test, feature = "arbitrary"))]
mod fuzz_tests {
    use super::*;
    use alloy_primitives::Bytes;
    use alloy_rlp::{Decodable, Encodable};
    use arbitrary::{Arbitrary, Unstructured};

    #[test]
    fn fuzz_decode_sealed_block_roundtrip() {
        let mut data = [0u8; 8192];
        for (i, byte) in data.iter_mut().enumerate() {
            *byte = (i.wrapping_mul(31).wrapping_add(17)) as u8;
        }

        let mut success_count = 0;
        for offset in 0..200 {
            let slice = &data[offset..];
            let mut u = Unstructured::new(slice);

            if let Ok(block) = Block::<Bytes, Header>::arbitrary(&mut u) {
                let expected_hash = block.header.hash_slow();

                let mut encoded = Vec::new();
                block.encode(&mut encoded);

                let mut buf = encoded.as_slice();
                if Block::<Bytes>::decode(&mut buf).is_ok() {
                    let mut buf = encoded.as_slice();
                    let sealed = Block::<Bytes>::decode_sealed(&mut buf).unwrap();

                    assert_eq!(sealed.hash(), expected_hash);
                    assert_eq!(*sealed.inner(), block);
                    success_count += 1;
                }
            }
        }
        assert!(success_count > 0, "No blocks were successfully tested");
    }

    #[test]
    fn fuzz_header_decode_sealed_roundtrip() {
        let mut data = [0u8; 4096];
        for (i, byte) in data.iter_mut().enumerate() {
            *byte = (i.wrapping_mul(37).wrapping_add(23)) as u8;
        }

        let mut success_count = 0;
        for offset in 0..200 {
            let slice = &data[offset..];
            let mut u = Unstructured::new(slice);

            if let Ok(header) = Header::arbitrary(&mut u) {
                let expected_hash = header.hash_slow();

                let mut encoded = Vec::new();
                header.encode(&mut encoded);

                let mut buf = encoded.as_slice();
                if Header::decode(&mut buf).is_ok() {
                    let mut buf = encoded.as_slice();
                    let sealed = Header::decode_sealed(&mut buf).unwrap();

                    assert_eq!(sealed.hash(), expected_hash);
                    assert_eq!(*sealed.inner(), header);
                    success_count += 1;
                }
            }
        }
        assert!(success_count > 0, "No headers were successfully tested");
    }
}
