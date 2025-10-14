#![cfg_attr(not(feature = "std"), no_std)]
extern crate alloc;

use alloc::vec::Vec;
use alloc::{fmt::Debug, sync::Arc};
use alloy_consensus::Receipt as AlloyReceipt;
use alloy_consensus::{
    Eip2718EncodableReceipt, ReceiptWithBloom, RlpDecodableReceipt, RlpEncodableReceipt, TxReceipt,
    Sealed, SignableTransaction, Signed, Transaction as ConsensusTx, TxLegacy, Typed2718,
};
use alloy_eips::eip2718::{Decodable2718, Eip2718Error, Eip2718Result, Encodable2718};
use alloy_primitives::{keccak256, Address, Bytes, Signature, TxHash, TxKind, U256, B256};
use alloy_rlp::{Decodable, Encodable, Header};
use alloy_consensus::transaction::{RlpEcdsaDecodableTx, RlpEcdsaEncodableTx, TxHashRef};

use core::hash::{Hash, Hasher};
use core::ops::Deref;
use reth_primitives_traits::{InMemorySize, MaybeCompact, MaybeSerde, MaybeSerdeBincodeCompat, SignedTransaction};
use reth_primitives_traits::crypto::secp256k1::{recover_signer, recover_signer_unchecked};

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ArbReceipt {
    Legacy(AlloyReceipt),
    Eip1559(AlloyReceipt),
    Eip2930(AlloyReceipt),
    Eip7702(AlloyReceipt),
    Deposit(ArbDepositReceipt),
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ArbDepositReceipt;
impl reth_primitives_traits::serde_bincode_compat::RlpBincode for ArbReceipt {}
impl reth_primitives_traits::serde_bincode_compat::RlpBincode for ArbTransactionSigned {}

impl InMemorySize for ArbReceipt {
    fn size(&self) -> usize {
        0
    }
}



impl alloy_consensus::Typed2718 for ArbReceipt {
    fn is_legacy(&self) -> bool {
        matches!(self, ArbReceipt::Legacy(_))
    }
    fn ty(&self) -> u8 {
        self.tx_type().as_u8()
    }
}






impl ArbReceipt {
    pub const fn tx_type(&self) -> arb_alloy_consensus::tx::ArbTxType {
        match self {
            ArbReceipt::Legacy(_)
            | ArbReceipt::Eip2930(_)
            | ArbReceipt::Eip1559(_)
            | ArbReceipt::Eip7702(_) => arb_alloy_consensus::tx::ArbTxType::ArbitrumLegacyTx,
            ArbReceipt::Deposit(_) => arb_alloy_consensus::tx::ArbTxType::ArbitrumDepositTx,
        }
    }

    pub const fn as_receipt(&self) -> &AlloyReceipt {
        match self {
            ArbReceipt::Legacy(r)
            | ArbReceipt::Eip2930(r)
            | ArbReceipt::Eip1559(r)
            | ArbReceipt::Eip7702(r) => r,
            ArbReceipt::Deposit(_) => {
                unreachable!()
            }
        }
    }

    pub fn rlp_encoded_fields_length(&self, bloom: &alloy_primitives::Bloom) -> usize {
        match self {
            ArbReceipt::Legacy(r)
            | ArbReceipt::Eip2930(r)
            | ArbReceipt::Eip1559(r)
            | ArbReceipt::Eip7702(r) => r.rlp_encoded_fields_length_with_bloom(bloom),
            ArbReceipt::Deposit(_) => {
                alloy_rlp::Encodable::length(&alloy_consensus::Eip658Value::Eip658(true)) +
                    alloy_rlp::Encodable::length(&0u64) +
                    alloy_rlp::Encodable::length(&alloc::vec::Vec::<alloy_primitives::Log>::new())
            }
        }
    }

    pub fn rlp_encode_fields(&self, bloom: &alloy_primitives::Bloom, out: &mut dyn alloy_rlp::bytes::BufMut) {
        match self {
            ArbReceipt::Legacy(r)
            | ArbReceipt::Eip2930(r)
            | ArbReceipt::Eip1559(r)
            | ArbReceipt::Eip7702(r) => r.rlp_encode_fields_with_bloom(bloom, out),
            ArbReceipt::Deposit(_) => {
                alloy_consensus::Eip658Value::Eip658(true).encode(out);
                (0u64).encode(out);
                let logs: alloc::vec::Vec<alloy_primitives::Log> = alloc::vec::Vec::new();
                logs.encode(out);
            }
        }
    }

    pub fn rlp_header_inner(&self, bloom: &alloy_primitives::Bloom) -> alloy_rlp::Header {
        alloy_rlp::Header { list: true, payload_length: self.rlp_encoded_fields_length(bloom) }
    }

    pub fn rlp_encode_fields_without_bloom(&self, out: &mut dyn alloy_rlp::bytes::BufMut) {
        match self {
            ArbReceipt::Legacy(r)
            | ArbReceipt::Eip2930(r)
            | ArbReceipt::Eip1559(r)
            | ArbReceipt::Eip7702(r) => {
                r.status.encode(out);
                r.cumulative_gas_used.encode(out);
                r.logs.encode(out);
            }
            ArbReceipt::Deposit(_) => {
                alloy_consensus::Eip658Value::Eip658(true).encode(out);
                (0u64).encode(out);
                let logs: alloc::vec::Vec<alloy_primitives::Log> = alloc::vec::Vec::new();
                logs.encode(out);
            }
        }
    }

    pub fn rlp_encoded_fields_length_without_bloom(&self) -> usize {
        match self {
            ArbReceipt::Legacy(r)
            | ArbReceipt::Eip2930(r)
            | ArbReceipt::Eip1559(r)
            | ArbReceipt::Eip7702(r) => {
                r.status.length() + r.cumulative_gas_used.length() + r.logs.length()
            }
            ArbReceipt::Deposit(_) => {
                alloy_consensus::Eip658Value::Eip658(true).length() +
                    (0u64).length() +
                    alloc::vec::Vec::<alloy_primitives::Log>::new().length()
            }
        }
    }

    pub fn rlp_header_inner_without_bloom(&self) -> alloy_rlp::Header {
        alloy_rlp::Header { list: true, payload_length: self.rlp_encoded_fields_length_without_bloom() }
    }

    pub fn rlp_decode_inner(buf: &mut &[u8], tx_type: arb_alloy_consensus::tx::ArbTxType) -> alloy_rlp::Result<alloy_consensus::ReceiptWithBloom<Self>> {
        match tx_type {
            arb_alloy_consensus::tx::ArbTxType::ArbitrumDepositTx => {
                let header = alloy_rlp::Header::decode(buf)?;
                if !header.list {
                    return Err(alloy_rlp::Error::UnexpectedString);
                }
                let remaining = buf.len();
                let _status: alloy_consensus::Eip658Value = alloy_rlp::Decodable::decode(buf)?;
                let _cumu: u64 = alloy_rlp::Decodable::decode(buf)?;
                let _logs: alloc::vec::Vec<alloy_primitives::Log> = alloy_rlp::Decodable::decode(buf)?;
                if buf.len() + header.payload_length != remaining {
                    return Err(alloy_rlp::Error::UnexpectedLength);
                }
                Ok(alloy_consensus::ReceiptWithBloom {
                    receipt: ArbReceipt::Deposit(ArbDepositReceipt),
                    logs_bloom: alloy_primitives::Bloom::ZERO,
                })
            }
            _ => {
                let alloy_consensus::ReceiptWithBloom { receipt, logs_bloom } =
                    <AlloyReceipt as alloy_consensus::RlpDecodableReceipt>::rlp_decode_with_bloom(buf)?;
                Ok(alloy_consensus::ReceiptWithBloom { receipt: ArbReceipt::Legacy(receipt), logs_bloom })
            }
        }
    }

    pub fn rlp_decode_inner_without_bloom(buf: &mut &[u8], tx_type: arb_alloy_consensus::tx::ArbTxType) -> alloy_rlp::Result<Self> {
        let header = alloy_rlp::Header::decode(buf)?;
        if !header.list {
            return Err(alloy_rlp::Error::UnexpectedString);
        }
        let remaining = buf.len();
        let status: alloy_consensus::Eip658Value = alloy_rlp::Decodable::decode(buf)?;
        let cumulative_gas_used: u64 = alloy_rlp::Decodable::decode(buf)?;
        let logs: alloc::vec::Vec<alloy_primitives::Log> = alloy_rlp::Decodable::decode(buf)?;
        if buf.len() + header.payload_length != remaining {
            return Err(alloy_rlp::Error::UnexpectedLength);
        }
        match tx_type {
            arb_alloy_consensus::tx::ArbTxType::ArbitrumDepositTx => Ok(Self::Deposit(ArbDepositReceipt)),
            _ => Ok(Self::Legacy(AlloyReceipt { status, cumulative_gas_used, logs })),
        }
    }
}

impl alloy_consensus::Eip2718EncodableReceipt for ArbReceipt {
    fn eip2718_encoded_length_with_bloom(&self, bloom: &alloy_primitives::Bloom) -> usize {
        let inner_len = self.rlp_header_inner(bloom).length_with_payload();
        match self {
            ArbReceipt::Deposit(_) => 1 + inner_len,
            _ => inner_len,
        }
    }

    fn eip2718_encode_with_bloom(&self, bloom: &alloy_primitives::Bloom, out: &mut dyn alloy_rlp::bytes::BufMut) {
        if matches!(self, ArbReceipt::Deposit(_)) {
            out.put_u8(arb_alloy_consensus::tx::ArbTxType::ArbitrumDepositTx.as_u8());
        }
        self.rlp_header_inner(bloom).encode(out);
        self.rlp_encode_fields(bloom, out);
    }
}

impl alloy_consensus::RlpEncodableReceipt for ArbReceipt {
    fn rlp_encoded_length_with_bloom(&self, bloom: &alloy_primitives::Bloom) -> usize {
        let mut len = self.eip2718_encoded_length_with_bloom(bloom);
        if !matches!(self, ArbReceipt::Legacy(_)) {
            len += alloy_rlp::Header { list: false, payload_length: self.eip2718_encoded_length_with_bloom(bloom) }.length();
        }
        len
    }

    fn rlp_encode_with_bloom(&self, bloom: &alloy_primitives::Bloom, out: &mut dyn alloy_rlp::bytes::BufMut) {
        if !matches!(self, ArbReceipt::Legacy(_)) {
            alloy_rlp::Header { list: false, payload_length: self.eip2718_encoded_length_with_bloom(bloom) }.encode(out);
        }
        self.eip2718_encode_with_bloom(bloom, out);
    }
}
impl alloy_eips::Encodable2718 for ArbReceipt {
    fn encode_2718_len(&self) -> usize {
        let type_len = if matches!(self, ArbReceipt::Legacy(_)) { 0 } else { 1 };
        type_len + self.rlp_header_inner_without_bloom().length_with_payload()
    }

    fn encode_2718(&self, out: &mut dyn alloy_rlp::bytes::BufMut) {
        if !matches!(self, ArbReceipt::Legacy(_)) {
            out.put_u8(self.tx_type().as_u8());
        }
        self.rlp_header_inner_without_bloom().encode(out);
        self.rlp_encode_fields_without_bloom(out);
    }
}

impl alloy_eips::Decodable2718 for ArbReceipt {
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Eip2718Result<Self> {
        let tx_type = arb_alloy_consensus::tx::ArbTxType::from_u8(ty)
            .map_err(|_| Eip2718Error::UnexpectedType(ty))?;
        Ok(Self::rlp_decode_inner_without_bloom(buf, tx_type)?)
    }

    fn fallback_decode(buf: &mut &[u8]) -> Eip2718Result<Self> {
        Ok(Self::rlp_decode_inner_without_bloom(buf, arb_alloy_consensus::tx::ArbTxType::ArbitrumLegacyTx)?)
    }
}
 
impl alloy_consensus::RlpDecodableReceipt for ArbReceipt {
    fn rlp_decode_with_bloom(buf: &mut &[u8]) -> alloy_rlp::Result<alloy_consensus::ReceiptWithBloom<Self>> {
        let header_buf = &mut &**buf;
        let header = alloy_rlp::Header::decode(header_buf)?;
        if header.list {
            return ArbReceipt::rlp_decode_inner(buf, arb_alloy_consensus::tx::ArbTxType::ArbitrumLegacyTx);
        }
        *buf = *header_buf;
        let remaining = buf.len();
        let ty = u8::decode(buf)?;
        let tx_type = arb_alloy_consensus::tx::ArbTxType::from_u8(ty).map_err(|_| alloy_rlp::Error::Custom("unexpected arb receipt tx type"))?;
        let this = ArbReceipt::rlp_decode_inner(buf, tx_type)?;
        if buf.len() + header.payload_length != remaining {
            return Err(alloy_rlp::Error::UnexpectedLength);
        }
        Ok(this)
    }
}

impl alloy_rlp::Encodable for ArbReceipt {
    fn encode(&self, out: &mut dyn alloy_rlp::bytes::BufMut) {
        self.network_encode(out);
    }

    fn length(&self) -> usize {
        self.network_len()
    }
}

impl alloy_rlp::Decodable for ArbReceipt {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self::network_decode(buf)?)
    }
}




#[cfg(feature = "reth-codec")]
mod compact_receipt {
    use super::*;
    use alloc::borrow::Cow;
    use reth_codecs::Compact;

    #[derive(reth_codecs::CompactZstd)]
    #[reth_zstd(
        compressor = reth_zstd_compressors::RECEIPT_COMPRESSOR,
        decompressor = reth_zstd_compressors::RECEIPT_DECOMPRESSOR
    )]
    struct CompactArbReceipt<'a> {
        is_legacy_like: bool,
        success: bool,
        cumulative_gas_used: u64,
        #[expect(clippy::owned_cow)]
        logs: Cow<'a, Vec<alloy_primitives::Log>>,
    }

    impl<'a> From<&'a ArbReceipt> for CompactArbReceipt<'a> {
        fn from(receipt: &'a ArbReceipt) -> Self {
            match receipt {
                ArbReceipt::Legacy(r)
                | ArbReceipt::Eip2930(r)
                | ArbReceipt::Eip1559(r)
                | ArbReceipt::Eip7702(r) => Self {
                    is_legacy_like: true,
                    success: r.status().into(),
                    cumulative_gas_used: r.cumulative_gas_used(),
                    logs: Cow::Borrowed(&r.logs),
                },
                ArbReceipt::Deposit(_) => Self {
                    is_legacy_like: false,
                    success: true,
                    cumulative_gas_used: 0,
                    logs: Cow::Owned(Vec::new()),
                },
            }
        }
    }

    impl From<CompactArbReceipt<'_>> for ArbReceipt {
        fn from(r: CompactArbReceipt<'_>) -> Self {
            let inner = AlloyReceipt {
                status: alloy_consensus::Eip658Value::Eip658(r.success),
                cumulative_gas_used: r.cumulative_gas_used,
                logs: r.logs.into_owned(),
            };
            if r.is_legacy_like {
                ArbReceipt::Legacy(inner)
            } else {
                ArbReceipt::Deposit(ArbDepositReceipt)
            }
        }
    }

    impl Compact for ArbReceipt {
        fn to_compact<B>(&self, buf: &mut B) -> usize
        where
            B: bytes::BufMut + AsMut<[u8]>,
        {
            CompactArbReceipt::from(self).to_compact(buf)
        }

        fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
            let (r, buf) = CompactArbReceipt::from_compact(buf, len);
            (r.into(), buf)
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ArbTypedTransaction {
    Deposit(arb_alloy_consensus::tx::ArbDepositTx),
    Unsigned(arb_alloy_consensus::tx::ArbUnsignedTx),
    Contract(arb_alloy_consensus::tx::ArbContractTx),
    Retry(arb_alloy_consensus::tx::ArbRetryTx),
    SubmitRetryable(arb_alloy_consensus::tx::ArbSubmitRetryableTx),
    Internal(arb_alloy_consensus::tx::ArbInternalTx),

    Legacy(TxLegacy),
    Eip2930(alloy_consensus::TxEip2930),
    Eip1559(alloy_consensus::TxEip1559),
    Eip4844(alloy_consensus::TxEip4844),
    Eip7702(alloy_consensus::TxEip7702),
}

#[cfg(feature = "reth-codec")]
impl reth_codecs::Compact for ArbTypedTransaction {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        match self {
            ArbTypedTransaction::Legacy(tx) => {
                let mut tmp = alloc::vec::Vec::new();
                tx.encode(&mut tmp);
                buf.put_slice(&tmp);
                0
            }
            ArbTypedTransaction::Deposit(tx) => {
                buf.put_u8(arb_alloy_consensus::tx::ArbTxType::ArbitrumDepositTx.as_u8());
                tx.encode(buf);
                1
            }
            ArbTypedTransaction::Unsigned(tx) => {
                buf.put_u8(arb_alloy_consensus::tx::ArbTxType::ArbitrumUnsignedTx.as_u8());
                tx.encode(buf);
                1
            }
            ArbTypedTransaction::Contract(tx) => {
                buf.put_u8(arb_alloy_consensus::tx::ArbTxType::ArbitrumContractTx.as_u8());
                tx.encode(buf);
                1
            }
            ArbTypedTransaction::Retry(tx) => {
                buf.put_u8(arb_alloy_consensus::tx::ArbTxType::ArbitrumRetryTx.as_u8());
                tx.encode(buf);
                1
            }
            ArbTypedTransaction::SubmitRetryable(tx) => {
                buf.put_u8(arb_alloy_consensus::tx::ArbTxType::ArbitrumSubmitRetryableTx.as_u8());
                tx.encode(buf);
                1
            }
            ArbTypedTransaction::Internal(tx) => {
                buf.put_u8(arb_alloy_consensus::tx::ArbTxType::ArbitrumInternalTx.as_u8());
                tx.encode(buf);
                1
            }
            ArbTypedTransaction::Eip2930(tx) => {
                buf.put_u8(0x01);
                tx.encode(buf);
                2
            }
            ArbTypedTransaction::Eip1559(tx) => {
                buf.put_u8(0x02);
                tx.encode(buf);
                2
            }
            ArbTypedTransaction::Eip4844(tx) => {
                buf.put_u8(0x03);
                tx.encode(buf);
                2
            }
            ArbTypedTransaction::Eip7702(tx) => {
                buf.put_u8(0x04);
                tx.encode(buf);
                2
            }
        }
    }

    fn from_compact(buf: &[u8], tx_bits: usize) -> (Self, &[u8]) {
        let mut slice: &[u8] = buf;

        match tx_bits {
            0 => {
                let tx = <TxLegacy as alloy_rlp::Decodable>::decode(&mut slice).unwrap_or_default();
                return (ArbTypedTransaction::Legacy(tx), &buf[buf.len() - slice.len()..]);
            }
            1 => {
                if let Some(first) = slice.first().copied() {
                    if let Ok(kind) = arb_alloy_consensus::tx::ArbTxType::from_u8(first) {
                        let mut rest = &slice[1..];
                        let parsed = match kind {
                            arb_alloy_consensus::tx::ArbTxType::ArbitrumDepositTx => {
                                <arb_alloy_consensus::tx::ArbDepositTx as alloy_rlp::Decodable>::decode(&mut rest)
                                    .map(ArbTypedTransaction::Deposit)
                            }
                            arb_alloy_consensus::tx::ArbTxType::ArbitrumUnsignedTx => {
                                <arb_alloy_consensus::tx::ArbUnsignedTx as alloy_rlp::Decodable>::decode(&mut rest)
                                    .map(ArbTypedTransaction::Unsigned)
                            }
                            arb_alloy_consensus::tx::ArbTxType::ArbitrumContractTx => {
                                <arb_alloy_consensus::tx::ArbContractTx as alloy_rlp::Decodable>::decode(&mut rest)
                                    .map(ArbTypedTransaction::Contract)
                            }
                            arb_alloy_consensus::tx::ArbTxType::ArbitrumRetryTx => {
                                <arb_alloy_consensus::tx::ArbRetryTx as alloy_rlp::Decodable>::decode(&mut rest)
                                    .map(ArbTypedTransaction::Retry)
                            }
                            arb_alloy_consensus::tx::ArbTxType::ArbitrumSubmitRetryableTx => {
                                <arb_alloy_consensus::tx::ArbSubmitRetryableTx as alloy_rlp::Decodable>::decode(&mut rest)
                                    .map(ArbTypedTransaction::SubmitRetryable)
                            }
                            arb_alloy_consensus::tx::ArbTxType::ArbitrumInternalTx => {
                                <arb_alloy_consensus::tx::ArbInternalTx as alloy_rlp::Decodable>::decode(&mut rest)
                                    .map(ArbTypedTransaction::Internal)
                            }
                            arb_alloy_consensus::tx::ArbTxType::ArbitrumLegacyTx => {
                                <TxLegacy as alloy_rlp::Decodable>::decode(&mut slice).map(ArbTypedTransaction::Legacy)
                            }
                        };
                        if let Ok(tx) = parsed {
                            let consumed = buf.len() - rest.len() - 1;
                            return (tx, &buf[consumed..]);
                        }
                    }
                }
                let tx = <TxLegacy as alloy_rlp::Decodable>::decode(&mut slice).unwrap_or_default();
                return (ArbTypedTransaction::Legacy(tx), &buf[buf.len() - slice.len()..]);
            }
            2 => {
                if let Some(first) = slice.first().copied() {
                    let mut rest = &slice[1..];
                    let parsed = match first {
                        0x01 => <alloy_consensus::TxEip2930 as alloy_rlp::Decodable>::decode(&mut rest)
                            .map(ArbTypedTransaction::Eip2930),
                        0x02 => <alloy_consensus::TxEip1559 as alloy_rlp::Decodable>::decode(&mut rest)
                            .map(ArbTypedTransaction::Eip1559),
                        0x03 => <alloy_consensus::TxEip4844 as alloy_rlp::Decodable>::decode(&mut rest)
                            .map(ArbTypedTransaction::Eip4844),
                        0x04 => <alloy_consensus::TxEip7702 as alloy_rlp::Decodable>::decode(&mut rest)
                            .map(ArbTypedTransaction::Eip7702),
                        _ => Err(alloy_rlp::Error::Custom("unknown eth typed tx tag")),
                    };
                    if let Ok(tx) = parsed {
                        let consumed = buf.len() - rest.len() - 1;
                        return (tx, &buf[consumed..]);
                    }
                }
                let tx = <TxLegacy as alloy_rlp::Decodable>::decode(&mut slice).unwrap_or_default();
                return (ArbTypedTransaction::Legacy(tx), &buf[buf.len() - slice.len()..]);
            }
            _ => {
                let tx = <TxLegacy as alloy_rlp::Decodable>::decode(&mut slice).unwrap_or_default();
                return (ArbTypedTransaction::Legacy(tx), &buf[buf.len() - slice.len()..]);
            }
        }
    }
}

impl alloy_consensus::TxReceipt for ArbReceipt {
    type Log = alloy_primitives::Log;

    fn status_or_post_state(&self) -> alloy_consensus::Eip658Value {
        match self {
            ArbReceipt::Legacy(r)
            | ArbReceipt::Eip2930(r)
            | ArbReceipt::Eip1559(r)
            | ArbReceipt::Eip7702(r) => r.status_or_post_state(),
            ArbReceipt::Deposit(_) => alloy_consensus::Eip658Value::Eip658(true),
        }
    }

    fn status(&self) -> bool {
        match self {
            ArbReceipt::Legacy(r)
            | ArbReceipt::Eip2930(r)
            | ArbReceipt::Eip1559(r)
            | ArbReceipt::Eip7702(r) => r.status(),
            ArbReceipt::Deposit(_) => true,
        }
    }

    fn bloom(&self) -> alloy_primitives::Bloom {
        match self {
            ArbReceipt::Legacy(r)
            | ArbReceipt::Eip2930(r)
            | ArbReceipt::Eip1559(r)
            | ArbReceipt::Eip7702(r) => r.bloom(),
            ArbReceipt::Deposit(_) => alloy_primitives::Bloom::ZERO,
        }
    }

    fn cumulative_gas_used(&self) -> u64 {
        match self {
            ArbReceipt::Legacy(r)
            | ArbReceipt::Eip2930(r)
            | ArbReceipt::Eip1559(r)
            | ArbReceipt::Eip7702(r) => r.cumulative_gas_used(),
            ArbReceipt::Deposit(_) => 0,
        }
    }

    fn logs(&self) -> &[Self::Log] {
        match self {
            ArbReceipt::Legacy(r)
            | ArbReceipt::Eip2930(r)
            | ArbReceipt::Eip1559(r)
            | ArbReceipt::Eip7702(r) => r.logs(),
            ArbReceipt::Deposit(_) => &[],
        }
    }

    fn into_logs(self) -> alloc::vec::Vec<Self::Log> {
        match self {
            ArbReceipt::Legacy(r)
            | ArbReceipt::Eip2930(r)
            | ArbReceipt::Eip1559(r)
            | ArbReceipt::Eip7702(r) => r.logs,
            ArbReceipt::Deposit(_) => alloc::vec::Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq)]
pub struct ArbTransactionSigned {
    hash: reth_primitives_traits::sync::OnceLock<TxHash>,
    signature: Signature,
    transaction: ArbTypedTransaction,
    input_cache: reth_primitives_traits::sync::OnceLock<Bytes>,
}
#[cfg(feature = "serde")]
impl serde::Serialize for ArbTransactionSigned {
    fn serialize<S>(&self, serializer: S) -> core::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("ArbTransactionSigned", 2)?;
        state.serialize_field("signature", &self.signature)?;
        state.serialize_field("hash", self.tx_hash())?;
        state.end()
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for ArbTransactionSigned {
    fn deserialize<D>(deserializer: D) -> core::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Helper {
            signature: Signature,
            transaction_encoded_2718: alloy_primitives::Bytes,
        }
        let helper = Helper::deserialize(deserializer)?;
        let mut slice: &[u8] = helper.transaction_encoded_2718.as_ref();
        let parsed = if let Some(first) = slice.first().copied() {
            match arb_alloy_consensus::tx::ArbTxType::from_u8(first) {
                Ok(_) => {
                    let mut rest = &slice[1..];
                    ArbTransactionSigned::typed_decode(first, &mut rest)
                        .map_err(serde::de::Error::custom)?
                }
                Err(_) => {
                    ArbTransactionSigned::fallback_decode(&mut slice)
                        .map_err(serde::de::Error::custom)?
                }
            }
        } else {
            return Err(serde::de::Error::custom("empty transaction_encoded_2718"));
        };
        let mut out = ArbTransactionSigned::new_unhashed(parsed.transaction, parsed.signature);
        out.recalculate_hash();
        if out.signature != helper.signature {
            return Err(serde::de::Error::custom("signature mismatch"));
        }
        Ok(out)
    }
}


impl Deref for ArbTransactionSigned {
    type Target = ArbTypedTransaction;
    fn deref(&self) -> &Self::Target {
        &self.transaction
    }
}

impl ArbTransactionSigned {
    pub fn new(transaction: ArbTypedTransaction, signature: Signature, hash: B256) -> Self {
        Self { hash: hash.into(), signature, transaction, input_cache: Default::default() }
    }

    pub fn new_unhashed(transaction: ArbTypedTransaction, signature: Signature) -> Self {
        Self { hash: Default::default(), signature, transaction, input_cache: Default::default() }
    }
    
    pub const fn signature(&self) -> &Signature {
        &self.signature
    }

    pub const fn tx_type(&self) -> ArbTxType {
        match &self.transaction {
            ArbTypedTransaction::Deposit(_) => ArbTxType::Deposit,
            ArbTypedTransaction::Unsigned(_) => ArbTxType::Unsigned,
            ArbTypedTransaction::Contract(_) => ArbTxType::Contract,
            ArbTypedTransaction::Retry(_) => ArbTxType::Retry,
            ArbTypedTransaction::SubmitRetryable(_) => ArbTxType::SubmitRetryable,
            ArbTypedTransaction::Internal(_) => ArbTxType::Internal,
            ArbTypedTransaction::Legacy(_) => ArbTxType::Legacy,
            ArbTypedTransaction::Eip2930(_) => ArbTxType::Eip2930,
            ArbTypedTransaction::Eip1559(_) => ArbTxType::Eip1559,
            ArbTypedTransaction::Eip4844(_) => ArbTxType::Eip4844,
            ArbTypedTransaction::Eip7702(_) => ArbTxType::Eip7702,
        }
    }

    pub(crate) fn recalculate_hash(&self) -> B256 {
        keccak256(self.encoded_2718())
    }

    pub fn split(self) -> (ArbTypedTransaction, Signature, B256) {
        let hash = *self.hash.get_or_init(|| self.recalculate_hash());
        (self.transaction, self.signature, hash)
    }
}

impl alloy_consensus::transaction::SignerRecoverable for ArbTransactionSigned {
    fn recover_signer(&self) -> Result<Address, reth_primitives_traits::transaction::signed::RecoveryError> {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => {
                let mut tmp = alloc::vec::Vec::new();
                tx.encode_for_signing(&mut tmp);
                let sig_hash = keccak256(&tmp);
                recover_signer(&self.signature, sig_hash)
            }
            ArbTypedTransaction::Deposit(tx) => Ok(tx.from),
            ArbTypedTransaction::Unsigned(tx) => Ok(tx.from),
            ArbTypedTransaction::Contract(tx) => Ok(tx.from),
            ArbTypedTransaction::Retry(tx) => Ok(tx.from),
            ArbTypedTransaction::SubmitRetryable(tx) => Ok(tx.from),
            ArbTypedTransaction::Internal(_) => Ok(alloy_primitives::address!("0x00000000000000000000000000000000000A4B05")),
            ArbTypedTransaction::Eip2930(tx) => {
                let mut tmp = alloc::vec::Vec::new();
                tx.encode_for_signing(&mut tmp);
                let sig_hash = keccak256(&tmp);
                recover_signer(&self.signature, sig_hash)
            }
            ArbTypedTransaction::Eip1559(tx) => {
                let mut tmp = alloc::vec::Vec::new();
                tx.encode_for_signing(&mut tmp);
                let sig_hash = keccak256(&tmp);
                recover_signer(&self.signature, sig_hash)
            }
            ArbTypedTransaction::Eip4844(tx) => {
                let mut tmp = alloc::vec::Vec::new();
                tx.encode_for_signing(&mut tmp);
                let sig_hash = keccak256(&tmp);
                recover_signer(&self.signature, sig_hash)
            }
            ArbTypedTransaction::Eip7702(tx) => {
                let mut tmp = alloc::vec::Vec::new();
                tx.encode_for_signing(&mut tmp);
                let sig_hash = keccak256(&tmp);
                recover_signer(&self.signature, sig_hash)
            }
        }
    }

    fn recover_signer_unchecked(&self) -> Result<Address, reth_primitives_traits::transaction::signed::RecoveryError> {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => {
                let mut tmp = alloc::vec::Vec::new();
                tx.encode_for_signing(&mut tmp);
                let sig_hash = keccak256(&tmp);
                recover_signer_unchecked(&self.signature, sig_hash)
            }
            ArbTypedTransaction::Deposit(tx) => Ok(tx.from),
            ArbTypedTransaction::Unsigned(tx) => Ok(tx.from),
            ArbTypedTransaction::Contract(tx) => Ok(tx.from),
            ArbTypedTransaction::Retry(tx) => Ok(tx.from),
            ArbTypedTransaction::SubmitRetryable(tx) => Ok(tx.from),
            ArbTypedTransaction::Internal(_) => Ok(alloy_primitives::address!("0x00000000000000000000000000000000000A4B05")),
            ArbTypedTransaction::Eip2930(tx) => {
                let mut tmp = alloc::vec::Vec::new();
                tx.encode_for_signing(&mut tmp);
                let sig_hash = keccak256(&tmp);
                recover_signer_unchecked(&self.signature, sig_hash)
            }
            ArbTypedTransaction::Eip1559(tx) => {
                let mut tmp = alloc::vec::Vec::new();
                tx.encode_for_signing(&mut tmp);
                let sig_hash = keccak256(&tmp);
                recover_signer_unchecked(&self.signature, sig_hash)
            }
            ArbTypedTransaction::Eip4844(tx) => {
                let mut tmp = alloc::vec::Vec::new();
                tx.encode_for_signing(&mut tmp);
                let sig_hash = keccak256(&tmp);
                recover_signer_unchecked(&self.signature, sig_hash)
            }
            ArbTypedTransaction::Eip7702(tx) => {
                let mut tmp = alloc::vec::Vec::new();
                tx.encode_for_signing(&mut tmp);
                let sig_hash = keccak256(&tmp);
                recover_signer_unchecked(&self.signature, sig_hash)
            }
        }
    }
}


impl alloy_consensus::transaction::TxHashRef for ArbTransactionSigned {
    fn tx_hash(&self) -> &TxHash {
        self.hash.get_or_init(|| self.recalculate_hash())
    }
}

impl SignedTransaction for ArbTransactionSigned {
    fn recalculate_hash(&self) -> B256 {
        alloy_primitives::keccak256(self.encoded_2718())
    }
}


impl Hash for ArbTransactionSigned {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.tx_hash().hash(state)
    }
}

impl PartialEq for ArbTransactionSigned {
    fn eq(&self, other: &Self) -> bool {
        self.tx_hash() == other.tx_hash()
    }
}

impl InMemorySize for ArbTransactionSigned {
    fn size(&self) -> usize {
        core::mem::size_of::<TxHash>() + core::mem::size_of::<Signature>()
    }
}

#[cfg(feature = "reth-codec")]
impl reth_codecs::Compact for ArbTransactionSigned {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let start = buf.as_mut().len();

        buf.put_u8(0);

        let sig_bits = self.signature.to_compact(buf) as u8;

        let mut tx_buf = alloc::vec::Vec::with_capacity(256);
        let tx_bits = self.transaction.to_compact(&mut tx_buf) as u8;

        let use_zstd = tx_buf.len() >= 32;
        if use_zstd {
            if cfg!(feature = "std") {
                reth_zstd_compressors::TRANSACTION_COMPRESSOR.with(|compressor| {
                    let mut compressor = compressor.borrow_mut();
                    let compressed = compressor.compress(&tx_buf).expect("zstd compress");
                    buf.put_slice(&compressed);
                });
            } else {
                let mut compressor = reth_zstd_compressors::create_tx_compressor();
                let compressed = compressor.compress(&tx_buf).expect("zstd compress");
                buf.put_slice(&compressed);
            }
        } else {
            buf.put_slice(&tx_buf);
        }

        let flags = sig_bits | (tx_bits << 1) | ((use_zstd as u8) << 3);
        buf.as_mut()[start] = flags;

        buf.as_mut().len() - start
    }

    fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
        use bytes::Buf;

        let (head, tail) = buf.split_at(len);
        let mut slice: &[u8] = head;

        let bitflags = slice.get_u8() as usize;

        let sig_bit = bitflags & 1;
        let (signature, slice_after_sig) = Signature::from_compact(slice, sig_bit);
        let mut slice = slice_after_sig;

        let zstd_bit = bitflags >> 3;
        let tx_type_bits = (bitflags & 0b110) >> 1;

        let (transaction, _) = if zstd_bit != 0 {
            #[cfg(feature = "std")]
            {
                let decomp_vec: alloc::vec::Vec<u8> =
                    reth_zstd_compressors::TRANSACTION_DECOMPRESSOR.with(|decompressor| {
                        let mut decompressor = decompressor.borrow_mut();
                        decompressor.decompress(slice).to_vec()
                    });
                let (tx, _tmp_tail) = ArbTypedTransaction::from_compact(&decomp_vec[..], tx_type_bits);
                (tx, slice)
            }
            #[cfg(not(feature = "std"))]
            {
                let mut decompressor = reth_zstd_compressors::create_tx_decompressor();
                let decomp_vec: alloc::vec::Vec<u8> = decompressor.decompress(slice).to_vec();
                let (tx, _tmp_tail) = ArbTypedTransaction::from_compact(&decomp_vec[..], tx_type_bits);
                (tx, slice)
            }
        } else {
            ArbTypedTransaction::from_compact(slice, tx_type_bits)
        };

        (Self { hash: Default::default(), signature, transaction, input_cache: Default::default() }, tail)
    }
}


impl Encodable for ArbTransactionSigned {
    fn encode(&self, out: &mut dyn alloy_rlp::bytes::BufMut) {
        self.network_encode(out);
    }
    fn length(&self) -> usize {
        let mut payload_length = self.encode_2718_len();
        if !Typed2718::is_legacy(self) {
            payload_length += alloy_rlp::Header { list: false, payload_length }.length();
        }
        payload_length
    }
}

impl Decodable for ArbTransactionSigned {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Self::network_decode(buf).map_err(Into::into)
    }
}
impl alloy_consensus::Typed2718 for ArbTransactionSigned {
    fn is_legacy(&self) -> bool {
        matches!(self.transaction, ArbTypedTransaction::Legacy(_))
    }
    fn ty(&self) -> u8 {
        match &self.transaction {
            ArbTypedTransaction::Legacy(_) => 0u8,
            ArbTypedTransaction::Deposit(_) => arb_alloy_consensus::tx::ArbTxType::ArbitrumDepositTx.as_u8(),
            ArbTypedTransaction::Unsigned(_) => arb_alloy_consensus::tx::ArbTxType::ArbitrumUnsignedTx.as_u8(),
            ArbTypedTransaction::Contract(_) => arb_alloy_consensus::tx::ArbTxType::ArbitrumContractTx.as_u8(),
            ArbTypedTransaction::Retry(_) => arb_alloy_consensus::tx::ArbTxType::ArbitrumRetryTx.as_u8(),
            ArbTypedTransaction::SubmitRetryable(_) => arb_alloy_consensus::tx::ArbTxType::ArbitrumSubmitRetryableTx.as_u8(),
            ArbTypedTransaction::Internal(_) => arb_alloy_consensus::tx::ArbTxType::ArbitrumInternalTx.as_u8(),
            ArbTypedTransaction::Eip2930(_) => 0x01,
            ArbTypedTransaction::Eip1559(_) => 0x02,
            ArbTypedTransaction::Eip4844(_) => 0x03,
            ArbTypedTransaction::Eip7702(_) => 0x04,
        }
    }
}

impl Encodable2718 for ArbTransactionSigned {
    fn type_flag(&self) -> Option<u8> {
        match &self.transaction {
            ArbTypedTransaction::Legacy(_) => None,

            _ => Some(match &self.transaction {
                ArbTypedTransaction::Deposit(_) => arb_alloy_consensus::tx::ArbTxType::ArbitrumDepositTx.as_u8(),
                ArbTypedTransaction::Unsigned(_) => arb_alloy_consensus::tx::ArbTxType::ArbitrumUnsignedTx.as_u8(),
                ArbTypedTransaction::Contract(_) => arb_alloy_consensus::tx::ArbTxType::ArbitrumContractTx.as_u8(),
                ArbTypedTransaction::Retry(_) => arb_alloy_consensus::tx::ArbTxType::ArbitrumRetryTx.as_u8(),
                ArbTypedTransaction::SubmitRetryable(_) => arb_alloy_consensus::tx::ArbTxType::ArbitrumSubmitRetryableTx.as_u8(),
                ArbTypedTransaction::Internal(_) => arb_alloy_consensus::tx::ArbTxType::ArbitrumInternalTx.as_u8(),
                ArbTypedTransaction::Eip2930(_) => 0x01,
                ArbTypedTransaction::Eip1559(_) => 0x02,
                ArbTypedTransaction::Eip4844(_) => 0x03,
                ArbTypedTransaction::Eip7702(_) => 0x04,
                ArbTypedTransaction::Legacy(_) => 0,
            }),
        }
    }

    fn encode_2718_len(&self) -> usize {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => tx.eip2718_encoded_length(&self.signature),
            ArbTypedTransaction::Deposit(tx) => tx.length() + 1,
            ArbTypedTransaction::Unsigned(tx) => tx.length() + 1,
            ArbTypedTransaction::Contract(tx) => tx.length() + 1,
            ArbTypedTransaction::Retry(tx) => tx.length() + 1,
            ArbTypedTransaction::SubmitRetryable(tx) => tx.length() + 1,
            ArbTypedTransaction::Internal(tx) => tx.length() + 1,

            ArbTypedTransaction::Eip2930(tx) => tx.eip2718_encoded_length(&self.signature),
            ArbTypedTransaction::Eip1559(tx) => tx.eip2718_encoded_length(&self.signature),
            ArbTypedTransaction::Eip4844(tx) => tx.eip2718_encoded_length(&self.signature),
            ArbTypedTransaction::Eip7702(tx) => tx.eip2718_encoded_length(&self.signature),
        }
    }

    fn encode_2718(&self, out: &mut dyn alloy_rlp::bytes::BufMut) {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => {
                tx.eip2718_encode(&self.signature, out)
            }
            ArbTypedTransaction::Deposit(tx) => {
                out.put_u8(arb_alloy_consensus::tx::ArbTxType::ArbitrumDepositTx.as_u8());
                tx.encode(out);
            }
            ArbTypedTransaction::Unsigned(tx) => {
                out.put_u8(arb_alloy_consensus::tx::ArbTxType::ArbitrumUnsignedTx.as_u8());
                tx.encode(out);
            }
            ArbTypedTransaction::Contract(tx) => {
                out.put_u8(arb_alloy_consensus::tx::ArbTxType::ArbitrumContractTx.as_u8());
                tx.encode(out);
            }
            ArbTypedTransaction::Retry(tx) => {
                out.put_u8(arb_alloy_consensus::tx::ArbTxType::ArbitrumRetryTx.as_u8());
                tx.encode(out);
            }
            ArbTypedTransaction::SubmitRetryable(tx) => {
                out.put_u8(arb_alloy_consensus::tx::ArbTxType::ArbitrumSubmitRetryableTx.as_u8());
                tx.encode(out);
            }
            ArbTypedTransaction::Internal(tx) => {
                out.put_u8(arb_alloy_consensus::tx::ArbTxType::ArbitrumInternalTx.as_u8());
                tx.encode(out);
            }

            ArbTypedTransaction::Eip2930(tx) => {
                tx.eip2718_encode(&self.signature, out)
            }
            ArbTypedTransaction::Eip1559(tx) => {
                tx.eip2718_encode(&self.signature, out)
            }
            ArbTypedTransaction::Eip4844(tx) => {
                tx.eip2718_encode(&self.signature, out)
            }
            ArbTypedTransaction::Eip7702(tx) => {
                tx.eip2718_encode(&self.signature, out)
            }
        }
    }
}

impl Decodable2718 for ArbTransactionSigned {
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Eip2718Result<Self> {
        if let Ok(kind) = arb_alloy_consensus::tx::ArbTxType::from_u8(ty) {
            return Ok(match kind {
                arb_alloy_consensus::tx::ArbTxType::ArbitrumDepositTx => {
                    let tx = arb_alloy_consensus::tx::ArbDepositTx::decode(buf)?;
                    Self::new_unhashed(ArbTypedTransaction::Deposit(tx), Signature::new(U256::ZERO, U256::ZERO, false))
                }
                arb_alloy_consensus::tx::ArbTxType::ArbitrumUnsignedTx => {
                    let tx = arb_alloy_consensus::tx::ArbUnsignedTx::decode(buf)?;
                    Self::new_unhashed(ArbTypedTransaction::Unsigned(tx), Signature::new(U256::ZERO, U256::ZERO, false))
                }
                arb_alloy_consensus::tx::ArbTxType::ArbitrumContractTx => {
                    let tx = arb_alloy_consensus::tx::ArbContractTx::decode(buf)?;
                    Self::new_unhashed(ArbTypedTransaction::Contract(tx), Signature::new(U256::ZERO, U256::ZERO, false))
                }
                arb_alloy_consensus::tx::ArbTxType::ArbitrumRetryTx => {
                    let tx = arb_alloy_consensus::tx::ArbRetryTx::decode(buf)?;
                    Self::new_unhashed(ArbTypedTransaction::Retry(tx), Signature::new(U256::ZERO, U256::ZERO, false))
                }
                arb_alloy_consensus::tx::ArbTxType::ArbitrumSubmitRetryableTx => {
                    let tx = arb_alloy_consensus::tx::ArbSubmitRetryableTx::decode(buf)?;
                    Self::new_unhashed(ArbTypedTransaction::SubmitRetryable(tx), Signature::new(U256::ZERO, U256::ZERO, false))
                }
                arb_alloy_consensus::tx::ArbTxType::ArbitrumInternalTx => {
                    let tx = arb_alloy_consensus::tx::ArbInternalTx::decode(buf)?;
                    Self::new_unhashed(ArbTypedTransaction::Internal(tx), Signature::new(U256::ZERO, U256::ZERO, false))
                }
                arb_alloy_consensus::tx::ArbTxType::ArbitrumLegacyTx => return Err(Eip2718Error::UnexpectedType(0x78)),
            });
        }

        match alloy_consensus::TxType::try_from(ty).map_err(|_| Eip2718Error::UnexpectedType(ty))? {
            alloy_consensus::TxType::Legacy => Err(Eip2718Error::UnexpectedType(0)),
            alloy_consensus::TxType::Eip2930 => {
                let (tx, signature) = alloy_consensus::TxEip2930::rlp_decode_with_signature(buf)?;
                Ok(Self { hash: Default::default(), signature, transaction: ArbTypedTransaction::Eip2930(tx), input_cache: Default::default() })
            }
            alloy_consensus::TxType::Eip1559 => {
                let (tx, signature) = alloy_consensus::TxEip1559::rlp_decode_with_signature(buf)?;
                Ok(Self { hash: Default::default(), signature, transaction: ArbTypedTransaction::Eip1559(tx), input_cache: Default::default() })
            }
            alloy_consensus::TxType::Eip4844 => {
                let (tx, signature) = alloy_consensus::TxEip4844::rlp_decode_with_signature(buf)?;
                Ok(Self { hash: Default::default(), signature, transaction: ArbTypedTransaction::Eip4844(tx), input_cache: Default::default() })
            }
            alloy_consensus::TxType::Eip7702 => {
                let (tx, signature) = alloy_consensus::TxEip7702::rlp_decode_with_signature(buf)?;
                Ok(Self { hash: Default::default(), signature, transaction: ArbTypedTransaction::Eip7702(tx), input_cache: Default::default() })
            }
        }
    }

    fn fallback_decode(buf: &mut &[u8]) -> Eip2718Result<Self> {
        let (tx, signature, hash) = TxLegacy::rlp_decode_signed(buf)?.into_parts();
        let mut signed_tx = Self::new_unhashed(ArbTypedTransaction::Legacy(tx), signature);
        signed_tx.hash.get_or_init(|| hash);
        Ok(signed_tx)
    }
}

impl ConsensusTx for ArbTransactionSigned {
    fn chain_id(&self) -> Option<u64> {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => tx.chain_id.map(|id| id as u64),
            ArbTypedTransaction::Deposit(tx) => Some(tx.chain_id.to::<u64>()),
            ArbTypedTransaction::Unsigned(tx) => Some(tx.chain_id.to::<u64>()),
            ArbTypedTransaction::Contract(tx) => Some(tx.chain_id.to::<u64>()),
            ArbTypedTransaction::Retry(tx) => Some(tx.chain_id.to::<u64>()),
            ArbTypedTransaction::SubmitRetryable(tx) => Some(tx.chain_id.to::<u64>()),
            ArbTypedTransaction::Internal(tx) => Some(tx.chain_id.to::<u64>()),

            ArbTypedTransaction::Eip2930(tx) => Some(tx.chain_id as u64),
            ArbTypedTransaction::Eip1559(tx) => Some(tx.chain_id as u64),
            ArbTypedTransaction::Eip4844(tx) => Some(tx.chain_id as u64),
            ArbTypedTransaction::Eip7702(tx) => Some(tx.chain_id as u64),
        }
    }

    fn nonce(&self) -> u64 {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => tx.nonce,
            ArbTypedTransaction::Deposit(_) => 0,
            ArbTypedTransaction::Unsigned(tx) => tx.nonce,
            ArbTypedTransaction::Contract(_) => 0,
            ArbTypedTransaction::Retry(tx) => tx.nonce,
            ArbTypedTransaction::SubmitRetryable(_) => 0,
            ArbTypedTransaction::Internal(_) => 0,

            ArbTypedTransaction::Eip2930(tx) => tx.nonce,
            ArbTypedTransaction::Eip1559(tx) => tx.nonce,
            ArbTypedTransaction::Eip4844(tx) => tx.nonce,
            ArbTypedTransaction::Eip7702(tx) => tx.nonce,
        }
    }

    fn gas_limit(&self) -> u64 {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => tx.gas_limit,
            ArbTypedTransaction::Deposit(_) => 0,
            ArbTypedTransaction::Unsigned(tx) => tx.gas,
            ArbTypedTransaction::Contract(tx) => tx.gas,
            ArbTypedTransaction::Retry(tx) => tx.gas,
            ArbTypedTransaction::SubmitRetryable(tx) => tx.gas,
            ArbTypedTransaction::Internal(_) => 0,

            ArbTypedTransaction::Eip2930(tx) => tx.gas_limit,
            ArbTypedTransaction::Eip1559(tx) => tx.gas_limit,
            ArbTypedTransaction::Eip4844(tx) => tx.gas_limit,
            ArbTypedTransaction::Eip7702(tx) => tx.gas_limit,
        }
    }

    fn gas_price(&self) -> Option<u128> {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => Some(tx.gas_price.into()),
            ArbTypedTransaction::Eip2930(_) => None,
            ArbTypedTransaction::Eip1559(_) => None,
            ArbTypedTransaction::Eip4844(_) => None,
            ArbTypedTransaction::Eip7702(_) => None,
            _ => None,
        }
    }

    fn max_fee_per_gas(&self) -> u128 {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => tx.gas_price.into(),
            ArbTypedTransaction::Unsigned(tx) => tx.gas_fee_cap.to::<u128>(),
            ArbTypedTransaction::Contract(tx) => tx.gas_fee_cap.to::<u128>(),
            ArbTypedTransaction::Retry(tx) => tx.gas_fee_cap.to::<u128>(),
            ArbTypedTransaction::SubmitRetryable(tx) => tx.gas_fee_cap.to::<u128>(),

            ArbTypedTransaction::Eip2930(_) => 0,
            ArbTypedTransaction::Eip1559(tx) => tx.max_fee_per_gas as u128,
            ArbTypedTransaction::Eip4844(tx) => tx.max_fee_per_gas as u128,
            ArbTypedTransaction::Eip7702(_) => 0,

            _ => 0,
        }
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        Some(0)
    }

    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        Some(0)
    }

    fn priority_fee_or_price(&self) -> u128 {
        self.gas_price().unwrap_or(0)
    }

    fn effective_gas_price(&self, _base_fee: Option<u64>) -> u128 {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => tx.gas_price.into(),
            ArbTypedTransaction::Eip1559(tx) => core::cmp::min(tx.max_fee_per_gas as u128, (tx.max_priority_fee_per_gas as u128) + _base_fee.unwrap_or(0) as u128),
            ArbTypedTransaction::Eip4844(tx) => core::cmp::min(tx.max_fee_per_gas as u128, (tx.max_priority_fee_per_gas as u128) + _base_fee.unwrap_or(0) as u128),
            _ => self.max_fee_per_gas(),
        }
    }

    fn effective_tip_per_gas(&self, _base_fee: u64) -> Option<u128> {
        Some(0)
    }

    fn is_dynamic_fee(&self) -> bool {
        !matches!(self.transaction, ArbTypedTransaction::Legacy(_) | ArbTypedTransaction::Eip2930(_))
    }

    fn kind(&self) -> TxKind {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => tx.to,
            ArbTypedTransaction::Deposit(tx) => if tx.to == Address::ZERO { TxKind::Create } else { TxKind::Call(tx.to) },
            ArbTypedTransaction::Unsigned(tx) => match tx.to {
                Some(to) => TxKind::Call(to),
                None => TxKind::Create,
            },
            ArbTypedTransaction::Contract(tx) => match tx.to {
                Some(to) => TxKind::Call(to),
                None => TxKind::Create,
            },
            ArbTypedTransaction::Retry(tx) => match tx.to {
                Some(to) => TxKind::Call(to),
                None => TxKind::Create,
            },
            ArbTypedTransaction::SubmitRetryable(_tx) => {
                let addr = alloy_primitives::address!("000000000000000000000000000000000000006e");
                TxKind::Call(addr)
            },
            ArbTypedTransaction::Internal(_) => TxKind::Call(alloy_primitives::address!("0xA4B05FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF")),
            ArbTypedTransaction::Eip2930(tx) => tx.to,
            ArbTypedTransaction::Eip1559(tx) => tx.to,
            ArbTypedTransaction::Eip4844(tx) => TxKind::Call(tx.to),
            ArbTypedTransaction::Eip7702(tx) => TxKind::Call(tx.to),
        }
    }

    fn is_create(&self) -> bool {
        matches!(self.kind(), TxKind::Create)
    }

    fn value(&self) -> U256 {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => tx.value,
            ArbTypedTransaction::Deposit(tx) => tx.value,
            ArbTypedTransaction::Unsigned(tx) => tx.value,
            ArbTypedTransaction::Contract(tx) => tx.value,
            ArbTypedTransaction::Retry(tx) => tx.value,
            ArbTypedTransaction::SubmitRetryable(tx) => tx.retry_value,
            ArbTypedTransaction::Internal(_) => U256::ZERO,
            ArbTypedTransaction::Eip2930(tx) => tx.value,
            ArbTypedTransaction::Eip1559(tx) => tx.value,
            ArbTypedTransaction::Eip4844(tx) => tx.value,
            ArbTypedTransaction::Eip7702(tx) => tx.value,
        }
    }

    fn input(&self) -> &Bytes {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => &tx.input,
            ArbTypedTransaction::Deposit(_) => {
                self.input_cache.get_or_init(|| Bytes::new())
            }
            ArbTypedTransaction::Unsigned(tx) => {
                self.input_cache.get_or_init(|| Bytes::from(tx.data.clone()))
            }
            ArbTypedTransaction::Contract(tx) => {
                self.input_cache.get_or_init(|| Bytes::from(tx.data.clone()))
            }
            ArbTypedTransaction::Retry(tx) => {
                self.input_cache.get_or_init(|| Bytes::from(tx.data.clone()))
            }
            ArbTypedTransaction::SubmitRetryable(tx) => {
                self.input_cache.get_or_init(|| {
                    let sel = arb_alloy_predeploys::selector(arb_alloy_predeploys::SIG_RETRY_SUBMIT_RETRYABLE);
                    let mut out = alloc::vec::Vec::with_capacity(4 + tx.retry_data.len());
                    out.extend_from_slice(&sel);
                    out.extend_from_slice(&tx.retry_data);
                    Bytes::from(out)
                })
            }
            ArbTypedTransaction::Internal(tx) => {
                self.input_cache.get_or_init(|| Bytes::from(tx.data.clone()))
            }

            ArbTypedTransaction::Eip2930(tx) => &tx.input,
            ArbTypedTransaction::Eip1559(tx) => &tx.input,
            ArbTypedTransaction::Eip4844(tx) => &tx.input,
            ArbTypedTransaction::Eip7702(tx) => &tx.input,
        }
    }

    fn access_list(&self) -> Option<&alloy_eips::eip2930::AccessList> {
        None
    }

    fn blob_versioned_hashes(&self) -> Option<&[B256]> {
        None
    }

    fn authorization_list(&self) -> Option<&[alloy_eips::eip7702::SignedAuthorization]> {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArbTxType {
    Deposit,
    Unsigned,
    Contract,
    Retry,
    SubmitRetryable,
    Internal,
    Legacy,
    Eip2930,
    Eip1559,
    Eip4844,
    Eip7702,
}


#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArbPrimitives;

impl reth_primitives_traits::NodePrimitives for ArbPrimitives {
    type Block = alloy_consensus::Block<ArbTransactionSigned, alloy_consensus::Header>;
    type BlockHeader = alloy_consensus::Header;
    type BlockBody = alloy_consensus::BlockBody<ArbTransactionSigned, alloy_consensus::Header>;
    type SignedTx = ArbTransactionSigned;
    type Receipt = ArbReceipt;
}

    #[test]
    fn decode_2718_exact_roundtrip_for_unsigned_tx() {
        use arb_alloy_consensus::tx::{ArbTxType as AType, ArbUnsignedTx};
        use alloy_primitives::{address, U256};
        use alloy_rlp::Encodable as RlpEncodable;

        let tx = ArbUnsignedTx {
            chain_id: U256::from(42161u64),
            from: address!("00000000000000000000000000000000000000aa"),
            nonce: 7,
            gas_fee_cap: U256::from(1_000_000u64),
            gas: 21000,
            to: Some(address!("00000000000000000000000000000000000000bb")),
            value: U256::from(123u64),
            data: alloc::vec::Vec::new().into(),
        };

        let mut enc = alloc::vec::Vec::with_capacity(1 + tx.length());
        enc.push(AType::ArbitrumUnsignedTx.as_u8());
        tx.encode(&mut enc);

        let signed = ArbTransactionSigned::decode_2718_exact(enc.as_slice()).expect("typed decode ok");
        assert_eq!(signed.tx_type(), ArbTxType::Unsigned);
        assert_eq!(signed.chain_id(), Some(42161));
        assert_eq!(signed.nonce(), 7);
        assert_eq!(signed.gas_limit(), 21000);
        assert_eq!(signed.value(), U256::from(123u64));
    }

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Eip658Value, Receipt};
    use alloy_primitives::Log;

    #[test]
    fn arb_receipt_variants_hold_alloy_receipt() {
        let r = Receipt { status: Eip658Value::Eip658(true), cumulative_gas_used: 1, logs: Vec::<Log>::new() };
        let e = ArbReceipt::Legacy(r.clone());
        match e {
            ArbReceipt::Legacy(rr) => {
                assert!(matches!(rr.status, Eip658Value::Eip658(true)));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn arb_deposit_receipt_variant_exists() {
        let d = ArbDepositReceipt::default();
        let e = ArbReceipt::Deposit(d);
        match e {
            ArbReceipt::Deposit(_) => {}
            _ => panic!("expected deposit variant"),
        }
    }
}
