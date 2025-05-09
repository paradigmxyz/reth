use alloy_consensus::{
    proofs::ordered_trie_root_with_encoder, Eip2718EncodableReceipt, Eip658Value, Receipt,
    ReceiptWithBloom, RlpDecodableReceipt, RlpEncodableReceipt, TxReceipt, Typed2718,
};
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Bloom, Log, B256, U256};
use alloy_rlp::{BufMut, Decodable, Header};
use reth_primitives_traits::InMemorySize;
use scroll_alloy_consensus::{ScrollTransactionReceipt, ScrollTxType};

/// Typed ethereum transaction receipt.
/// Receipt containing result of transaction execution.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub enum ScrollReceipt {
    /// Legacy receipt
    Legacy(ScrollTransactionReceipt),
    /// EIP-2930 receipt
    Eip2930(ScrollTransactionReceipt),
    /// EIP-1559 receipt
    Eip1559(ScrollTransactionReceipt),
    /// EIP-7702 receipt
    Eip7702(ScrollTransactionReceipt),
    /// L1 message receipt
    L1Message(Receipt),
}

impl ScrollReceipt {
    /// Returns [`ScrollTxType`] of the receipt.
    pub const fn tx_type(&self) -> ScrollTxType {
        match self {
            Self::Legacy(_) => ScrollTxType::Legacy,
            Self::Eip2930(_) => ScrollTxType::Eip2930,
            Self::Eip1559(_) => ScrollTxType::Eip1559,
            Self::Eip7702(_) => ScrollTxType::Eip7702,
            Self::L1Message(_) => ScrollTxType::L1Message,
        }
    }

    /// Returns inner [`Receipt`],
    pub const fn as_receipt(&self) -> &Receipt {
        match self {
            Self::Legacy(receipt) |
            Self::Eip2930(receipt) |
            Self::Eip1559(receipt) |
            Self::Eip7702(receipt) => &receipt.inner,
            Self::L1Message(receipt) => receipt,
        }
    }

    /// Returns length of RLP-encoded receipt fields with the given [`Bloom`] without an RLP header.
    pub fn rlp_encoded_fields_length(&self, bloom: &Bloom) -> usize {
        match self {
            Self::Legacy(receipt) |
            Self::Eip2930(receipt) |
            Self::Eip1559(receipt) |
            Self::Eip7702(receipt) => receipt.rlp_encoded_fields_length_with_bloom(bloom),
            Self::L1Message(receipt) => receipt.rlp_encoded_fields_length_with_bloom(bloom),
        }
    }

    /// RLP-encodes receipt fields with the given [`Bloom`] without an RLP header.
    pub fn rlp_encode_fields(&self, bloom: &Bloom, out: &mut dyn BufMut) {
        match self {
            Self::Legacy(receipt) |
            Self::Eip2930(receipt) |
            Self::Eip1559(receipt) |
            Self::Eip7702(receipt) => receipt.rlp_encode_fields_with_bloom(bloom, out),
            Self::L1Message(receipt) => receipt.rlp_encode_fields_with_bloom(bloom, out),
        }
    }

    /// Returns RLP header for inner encoding.
    pub fn rlp_header_inner(&self, bloom: &Bloom) -> Header {
        Header { list: true, payload_length: self.rlp_encoded_fields_length(bloom) }
    }

    /// RLP-decodes the receipt from the provided buffer. This does not expect a type byte or
    /// network header.
    pub fn rlp_decode_inner(
        buf: &mut &[u8],
        tx_type: ScrollTxType,
    ) -> alloy_rlp::Result<ReceiptWithBloom<Self>> {
        match tx_type {
            ScrollTxType::Legacy => {
                let ReceiptWithBloom { receipt, logs_bloom } =
                    RlpDecodableReceipt::rlp_decode_with_bloom(buf)?;
                Ok(ReceiptWithBloom { receipt: Self::Legacy(receipt), logs_bloom })
            }
            ScrollTxType::Eip2930 => {
                let ReceiptWithBloom { receipt, logs_bloom } =
                    RlpDecodableReceipt::rlp_decode_with_bloom(buf)?;
                Ok(ReceiptWithBloom { receipt: Self::Eip2930(receipt), logs_bloom })
            }
            ScrollTxType::Eip1559 => {
                let ReceiptWithBloom { receipt, logs_bloom } =
                    RlpDecodableReceipt::rlp_decode_with_bloom(buf)?;
                Ok(ReceiptWithBloom { receipt: Self::Eip1559(receipt), logs_bloom })
            }
            ScrollTxType::Eip7702 => {
                let ReceiptWithBloom { receipt, logs_bloom } =
                    RlpDecodableReceipt::rlp_decode_with_bloom(buf)?;
                Ok(ReceiptWithBloom { receipt: Self::Eip7702(receipt), logs_bloom })
            }
            ScrollTxType::L1Message => {
                let ReceiptWithBloom { receipt, logs_bloom } =
                    RlpDecodableReceipt::rlp_decode_with_bloom(buf)?;
                Ok(ReceiptWithBloom { receipt: Self::L1Message(receipt), logs_bloom })
            }
        }
    }

    /// Returns the l1 fee for the transaction receipt.
    pub const fn l1_fee(&self) -> U256 {
        match self {
            Self::Legacy(receipt) |
            Self::Eip2930(receipt) |
            Self::Eip1559(receipt) |
            Self::Eip7702(receipt) => receipt.l1_fee,
            Self::L1Message(_) => U256::ZERO,
        }
    }

    /// Calculates the receipt root for a header for the reference type of [Receipt].
    ///
    /// NOTE: Prefer `proofs::calculate_receipt_root` if you have log blooms memoized.
    pub fn calculate_receipt_root_no_memo(receipts: &[Self]) -> B256 {
        ordered_trie_root_with_encoder(receipts, |r, buf| r.with_bloom_ref().encode_2718(buf))
    }
}

impl Eip2718EncodableReceipt for ScrollReceipt {
    fn eip2718_encoded_length_with_bloom(&self, bloom: &Bloom) -> usize {
        !self.tx_type().is_legacy() as usize + self.rlp_header_inner(bloom).length_with_payload()
    }

    fn eip2718_encode_with_bloom(&self, bloom: &Bloom, out: &mut dyn BufMut) {
        if !self.tx_type().is_legacy() {
            out.put_u8(self.tx_type() as u8);
        }
        self.rlp_header_inner(bloom).encode(out);
        self.rlp_encode_fields(bloom, out);
    }
}

impl RlpEncodableReceipt for ScrollReceipt {
    fn rlp_encoded_length_with_bloom(&self, bloom: &Bloom) -> usize {
        let mut len = self.eip2718_encoded_length_with_bloom(bloom);
        if !self.tx_type().is_legacy() {
            len += Header {
                list: false,
                payload_length: self.eip2718_encoded_length_with_bloom(bloom),
            }
            .length();
        }

        len
    }

    fn rlp_encode_with_bloom(&self, bloom: &Bloom, out: &mut dyn BufMut) {
        if !self.tx_type().is_legacy() {
            Header { list: false, payload_length: self.eip2718_encoded_length_with_bloom(bloom) }
                .encode(out);
        }
        self.eip2718_encode_with_bloom(bloom, out);
    }
}

impl RlpDecodableReceipt for ScrollReceipt {
    fn rlp_decode_with_bloom(buf: &mut &[u8]) -> alloy_rlp::Result<ReceiptWithBloom<Self>> {
        let header_buf = &mut &**buf;
        let header = Header::decode(header_buf)?;

        // Legacy receipt, reuse initial buffer without advancing
        if header.list {
            return Self::rlp_decode_inner(buf, ScrollTxType::Legacy)
        }

        // Otherwise, advance the buffer and try decoding type flag followed by receipt
        *buf = *header_buf;

        let remaining = buf.len();
        let tx_type = ScrollTxType::decode(buf)?;
        let this = Self::rlp_decode_inner(buf, tx_type)?;

        if buf.len() + header.payload_length != remaining {
            return Err(alloy_rlp::Error::UnexpectedLength);
        }

        Ok(this)
    }
}

impl TxReceipt for ScrollReceipt {
    type Log = Log;

    fn status_or_post_state(&self) -> Eip658Value {
        self.as_receipt().status_or_post_state()
    }

    fn status(&self) -> bool {
        self.as_receipt().status()
    }

    fn bloom(&self) -> Bloom {
        self.as_receipt().bloom()
    }

    fn cumulative_gas_used(&self) -> u64 {
        self.as_receipt().cumulative_gas_used()
    }

    fn logs(&self) -> &[Log] {
        self.as_receipt().logs()
    }
}

impl Typed2718 for ScrollReceipt {
    fn ty(&self) -> u8 {
        self.tx_type().into()
    }
}

impl InMemorySize for ScrollReceipt {
    fn size(&self) -> usize {
        self.as_receipt().size()
    }
}

impl reth_primitives_traits::Receipt for ScrollReceipt {}

#[cfg(feature = "serde-bincode-compat")]
impl reth_primitives_traits::serde_bincode_compat::SerdeBincodeCompat for ScrollReceipt {
    type BincodeRepr<'a> = Self;

    fn as_repr(&self) -> Self::BincodeRepr<'_> {
        self.clone()
    }

    fn from_repr(repr: Self::BincodeRepr<'_>) -> Self {
        repr
    }
}

#[cfg(feature = "reth-codec")]
mod compact {
    use super::*;
    use alloc::borrow::Cow;
    use alloy_primitives::U256;
    use reth_codecs::Compact;

    #[derive(reth_codecs::CompactZstd)]
    #[reth_zstd(
        compressor = reth_zstd_compressors::RECEIPT_COMPRESSOR,
        decompressor = reth_zstd_compressors::RECEIPT_DECOMPRESSOR
    )]
    struct CompactScrollReceipt<'a> {
        tx_type: ScrollTxType,
        success: bool,
        cumulative_gas_used: u64,
        #[allow(clippy::owned_cow)]
        logs: Cow<'a, Vec<Log>>,
        l1_fee: Option<U256>,
    }

    impl<'a> From<&'a ScrollReceipt> for CompactScrollReceipt<'a> {
        fn from(receipt: &'a ScrollReceipt) -> Self {
            Self {
                tx_type: receipt.tx_type(),
                success: receipt.status(),
                cumulative_gas_used: receipt.cumulative_gas_used(),
                logs: Cow::Borrowed(&receipt.as_receipt().logs),
                l1_fee: (receipt.l1_fee() != U256::ZERO).then_some(receipt.l1_fee()),
            }
        }
    }

    impl From<CompactScrollReceipt<'_>> for ScrollReceipt {
        fn from(receipt: CompactScrollReceipt<'_>) -> Self {
            let CompactScrollReceipt { tx_type, success, cumulative_gas_used, logs, l1_fee } =
                receipt;

            let inner =
                Receipt { status: success.into(), cumulative_gas_used, logs: logs.into_owned() };

            match tx_type {
                ScrollTxType::Legacy => {
                    Self::Legacy(ScrollTransactionReceipt::new(inner, l1_fee.unwrap_or_default()))
                }
                ScrollTxType::Eip2930 => {
                    Self::Eip2930(ScrollTransactionReceipt::new(inner, l1_fee.unwrap_or_default()))
                }
                ScrollTxType::Eip1559 => {
                    Self::Eip1559(ScrollTransactionReceipt::new(inner, l1_fee.unwrap_or_default()))
                }
                ScrollTxType::Eip7702 => {
                    Self::Eip7702(ScrollTransactionReceipt::new(inner, l1_fee.unwrap_or_default()))
                }
                ScrollTxType::L1Message => Self::L1Message(inner),
            }
        }
    }

    impl Compact for ScrollReceipt {
        fn to_compact<B>(&self, buf: &mut B) -> usize
        where
            B: bytes::BufMut + AsMut<[u8]>,
        {
            CompactScrollReceipt::from(self).to_compact(buf)
        }

        fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
            let (receipt, buf) = CompactScrollReceipt::from_compact(buf, len);
            (receipt.into(), buf)
        }
    }
}
