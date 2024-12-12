use alloc::vec::Vec;
use alloy_consensus::{
    Eip2718EncodableReceipt, Eip658Value, ReceiptWithBloom, RlpDecodableReceipt,
    RlpEncodableReceipt, TxReceipt, Typed2718,
};
use alloy_primitives::{Bloom, Log};
use alloy_rlp::{BufMut, Decodable, Encodable, Header};
use op_alloy_consensus::OpTxType;
use reth_primitives_traits::InMemorySize;

/// Typed ethereum transaction receipt.
/// Receipt containing result of transaction execution.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "reth-codec", derive(reth_codecs::CompactZstd), reth_codecs::add_arbitrary_tests, reth_zstd(
    compressor = reth_zstd_compressors::RECEIPT_COMPRESSOR,
    decompressor = reth_zstd_compressors::RECEIPT_DECOMPRESSOR
))]
pub struct OpReceipt {
    /// Receipt type.
    pub tx_type: OpTxType,
    /// If transaction is executed successfully.
    ///
    /// This is the `statusCode`
    pub success: bool,
    /// Gas used
    pub cumulative_gas_used: u64,
    /// Log send from contracts.
    pub logs: Vec<Log>,
    /// Deposit nonce for Optimism deposit transactions
    pub deposit_nonce: Option<u64>,
    /// Deposit receipt version for Optimism deposit transactions
    ///
    ///
    /// The deposit receipt version was introduced in Canyon to indicate an update to how
    /// receipt hashes should be computed when set. The state transition process
    /// ensures this is only set for post-Canyon deposit transactions.
    pub deposit_receipt_version: Option<u64>,
}

impl OpReceipt {
    /// Returns length of RLP-encoded receipt fields with the given [`Bloom`] without an RLP header.
    pub fn rlp_encoded_fields_length(&self, bloom: &Bloom) -> usize {
        self.success.length() +
            self.cumulative_gas_used.length() +
            bloom.length() +
            self.logs.length() +
            self.deposit_nonce.map(|n| n.length()).unwrap_or(0) +
            self.deposit_receipt_version.map(|v| v.length()).unwrap_or(0)
    }

    /// RLP-encodes receipt fields with the given [`Bloom`] without an RLP header.
    pub fn rlp_encode_fields(&self, bloom: &Bloom, out: &mut dyn BufMut) {
        self.success.encode(out);
        self.cumulative_gas_used.encode(out);
        bloom.encode(out);
        self.logs.encode(out);
        if let Some(nonce) = self.deposit_nonce {
            nonce.encode(out);
        }
        if let Some(version) = self.deposit_receipt_version {
            version.encode(out);
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
        tx_type: OpTxType,
    ) -> alloy_rlp::Result<ReceiptWithBloom<Self>> {
        let header = Header::decode(buf)?;
        if !header.list {
            return Err(alloy_rlp::Error::UnexpectedString);
        }

        let remaining = buf.len();

        let success = Decodable::decode(buf)?;
        let cumulative_gas_used = Decodable::decode(buf)?;
        let logs_bloom = Decodable::decode(buf)?;
        let logs = Decodable::decode(buf)?;

        let (deposit_nonce, deposit_receipt_version) = if tx_type == OpTxType::Deposit {
            let remaining = || header.payload_length - (remaining - buf.len()) > 0;
            let deposit_nonce = remaining().then(|| Decodable::decode(buf)).transpose()?;
            let deposit_receipt_version =
                remaining().then(|| Decodable::decode(buf)).transpose()?;

            (deposit_nonce, deposit_receipt_version)
        } else {
            (None, None)
        };

        if buf.len() + header.payload_length != remaining {
            return Err(alloy_rlp::Error::UnexpectedLength);
        }

        Ok(ReceiptWithBloom {
            receipt: Self {
                cumulative_gas_used,
                tx_type,
                success,
                logs,
                deposit_nonce,
                deposit_receipt_version,
            },
            logs_bloom,
        })
    }
}

impl Eip2718EncodableReceipt for OpReceipt {
    fn eip2718_encoded_length_with_bloom(&self, bloom: &Bloom) -> usize {
        !self.tx_type.is_legacy() as usize + self.rlp_header_inner(bloom).length_with_payload()
    }

    fn eip2718_encode_with_bloom(&self, bloom: &Bloom, out: &mut dyn BufMut) {
        if !self.tx_type.is_legacy() {
            out.put_u8(self.tx_type as u8);
        }
        self.rlp_header_inner(bloom).encode(out);
        self.rlp_encode_fields(bloom, out);
    }
}

impl RlpEncodableReceipt for OpReceipt {
    fn rlp_encoded_length_with_bloom(&self, bloom: &Bloom) -> usize {
        let mut len = self.eip2718_encoded_length_with_bloom(bloom);
        if !self.tx_type.is_legacy() {
            len += Header {
                list: false,
                payload_length: self.eip2718_encoded_length_with_bloom(bloom),
            }
            .length();
        }

        len
    }

    fn rlp_encode_with_bloom(&self, bloom: &Bloom, out: &mut dyn BufMut) {
        if !self.tx_type.is_legacy() {
            Header { list: false, payload_length: self.eip2718_encoded_length_with_bloom(bloom) }
                .encode(out);
        }
        self.eip2718_encode_with_bloom(bloom, out);
    }
}

impl RlpDecodableReceipt for OpReceipt {
    fn rlp_decode_with_bloom(buf: &mut &[u8]) -> alloy_rlp::Result<ReceiptWithBloom<Self>> {
        let header_buf = &mut &**buf;
        let header = Header::decode(header_buf)?;

        // Legacy receipt, reuse initial buffer without advancing
        if header.list {
            return Self::rlp_decode_inner(buf, OpTxType::Legacy)
        }

        // Otherwise, advance the buffer and try decoding type flag followed by receipt
        *buf = *header_buf;

        let remaining = buf.len();
        let tx_type = OpTxType::decode(buf)?;
        let this = Self::rlp_decode_inner(buf, tx_type)?;

        if buf.len() + header.payload_length != remaining {
            return Err(alloy_rlp::Error::UnexpectedLength);
        }

        Ok(this)
    }
}

impl TxReceipt for OpReceipt {
    type Log = Log;

    fn status_or_post_state(&self) -> Eip658Value {
        self.success.into()
    }

    fn status(&self) -> bool {
        self.success
    }

    fn bloom(&self) -> Bloom {
        alloy_primitives::logs_bloom(self.logs())
    }

    fn cumulative_gas_used(&self) -> u128 {
        self.cumulative_gas_used as u128
    }

    fn logs(&self) -> &[Log] {
        &self.logs
    }
}

impl Typed2718 for OpReceipt {
    fn ty(&self) -> u8 {
        self.tx_type as u8
    }
}

impl InMemorySize for OpReceipt {
    fn size(&self) -> usize {
        self.tx_type.size() +
            core::mem::size_of::<bool>() +
            core::mem::size_of::<u64>() +
            self.logs.capacity() * core::mem::size_of::<Log>()
    }
}

impl reth_primitives_traits::Receipt for OpReceipt {}

#[cfg(feature = "arbitrary")]
impl arbitrary::Arbitrary<'_> for OpReceipt {
    fn arbitrary(u: &mut arbitrary::Unstructured<'_>) -> arbitrary::Result<Self> {
        let tx_type = u.arbitrary()?;

        let (deposit_nonce, deposit_receipt_version) = if tx_type == OpTxType::Deposit {
            let deposit_nonce: Option<u64> = u.arbitrary()?;
            let deposit_receipt_version =
                (deposit_nonce.is_some()).then(|| u.arbitrary()).transpose()?;

            (deposit_nonce, deposit_receipt_version)
        } else {
            (None, None)
        };

        Ok(Self {
            logs: u.arbitrary()?,
            success: u.arbitrary()?,
            cumulative_gas_used: u.arbitrary()?,
            tx_type,
            deposit_nonce,
            deposit_receipt_version,
        })
    }
}
