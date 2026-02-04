//! Compact implementations for Optimism types.

use crate::Compact;
use alloc::{borrow::Cow, vec::Vec};
use alloy_consensus::{Receipt, TxReceipt};
use alloy_primitives::Log;
use op_alloy_consensus::{OpDepositReceipt, OpReceipt, OpTxType};
use reth_codecs_derive::CompactZstd;

#[derive(CompactZstd)]
#[reth_codecs(crate = "crate")]
#[reth_zstd(
    compressor = reth_zstd_compressors::with_receipt_compressor,
    decompressor = reth_zstd_compressors::with_receipt_decompressor
)]
struct CompactOpReceipt<'a> {
    tx_type: OpTxType,
    success: bool,
    cumulative_gas_used: u64,
    #[expect(clippy::owned_cow)]
    logs: Cow<'a, Vec<Log>>,
    deposit_nonce: Option<u64>,
    deposit_receipt_version: Option<u64>,
}

impl<'a> From<&'a OpReceipt> for CompactOpReceipt<'a> {
    fn from(receipt: &'a OpReceipt) -> Self {
        Self {
            tx_type: receipt.tx_type(),
            success: receipt.status(),
            cumulative_gas_used: receipt.cumulative_gas_used(),
            logs: Cow::Borrowed(&receipt.as_receipt().logs),
            deposit_nonce: if let OpReceipt::Deposit(receipt) = receipt {
                receipt.deposit_nonce
            } else {
                None
            },
            deposit_receipt_version: if let OpReceipt::Deposit(receipt) = receipt {
                receipt.deposit_receipt_version
            } else {
                None
            },
        }
    }
}

impl From<CompactOpReceipt<'_>> for OpReceipt {
    fn from(receipt: CompactOpReceipt<'_>) -> Self {
        let CompactOpReceipt {
            tx_type,
            success,
            cumulative_gas_used,
            logs,
            deposit_nonce,
            deposit_receipt_version,
        } = receipt;

        let inner =
            Receipt { status: success.into(), cumulative_gas_used, logs: logs.into_owned() };

        match tx_type {
            OpTxType::Legacy => Self::Legacy(inner),
            OpTxType::Eip2930 => Self::Eip2930(inner),
            OpTxType::Eip1559 => Self::Eip1559(inner),
            OpTxType::Eip7702 => Self::Eip7702(inner),
            OpTxType::Deposit => {
                Self::Deposit(OpDepositReceipt { inner, deposit_nonce, deposit_receipt_version })
            }
        }
    }
}

impl Compact for OpReceipt {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        CompactOpReceipt::from(self).to_compact(buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (receipt, buf) = CompactOpReceipt::from_compact(buf, len);
        (receipt.into(), buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_utils::UnusedBits, validate_bitflag_backwards_compat};

    #[test]
    fn test_ensure_backwards_compatibility() {
        assert_eq!(CompactOpReceipt::bitflag_encoded_bytes(), 2);
        validate_bitflag_backwards_compat!(CompactOpReceipt<'_>, UnusedBits::NotZero);
    }
}
