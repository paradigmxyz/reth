//! Compact implementation for [`AlloyEthereumReceipt`]

use crate::Compact;
use alloc::vec::Vec;
use alloy_consensus::EthereumReceipt as AlloyEthereumReceipt;
use alloy_primitives::Log;
use bytes::Buf;
use modular_bitfield::prelude::*;

#[allow(non_snake_case)]
mod flags {
    use super::*;

    /// Bitflag fieldset for receipt compact encoding.
    ///
    /// Used bytes: 1 | Unused bits: 0
    #[bitfield]
    #[derive(Clone, Copy, Debug, Default)]
    pub struct ReceiptFlags {
        pub tx_type_len: B2,
        pub success_len: B1,
        pub cumulative_gas_used_len: B4,
        pub __zstd: B1,
    }

    impl ReceiptFlags {
        /// Deserializes this fieldset and returns it, alongside the original slice in an advanced
        /// position.
        pub fn from(mut buf: &[u8]) -> (Self, &[u8]) {
            (Self::from_bytes([buf.get_u8()]), buf)
        }
    }
}

pub(crate) use flags::ReceiptFlags;

impl<T: Compact> Compact for AlloyEthereumReceipt<T> {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let mut flags = ReceiptFlags::default();
        let mut total_length = 0;
        let mut buffer = bytes::BytesMut::new();

        let tx_type_len = self.tx_type.to_compact(&mut buffer);
        flags.set_tx_type_len(tx_type_len as u8);
        let success_len = self.success.to_compact(&mut buffer);
        flags.set_success_len(success_len as u8);
        let cumulative_gas_used_len = self.cumulative_gas_used.to_compact(&mut buffer);
        flags.set_cumulative_gas_used_len(cumulative_gas_used_len as u8);
        self.logs.to_compact(&mut buffer);

        let zstd = buffer.len() > 7;
        if zstd {
            flags.set___zstd(1);
        }

        let flags = flags.into_bytes();
        total_length += flags.len() + buffer.len();
        buf.put_slice(&flags);
        if zstd {
            reth_zstd_compressors::with_receipt_compressor(|compressor| {
                let compressed = compressor.compress(&buffer).expect("Failed to compress.");
                buf.put(compressed.as_slice());
            });
        } else {
            buf.put(buffer);
        }
        total_length
    }

    fn from_compact(buf: &[u8], _len: usize) -> (Self, &[u8]) {
        let (flags, mut buf) = ReceiptFlags::from(buf);
        if flags.__zstd() != 0 {
            reth_zstd_compressors::with_receipt_decompressor(|decompressor| {
                let decompressed = decompressor.decompress(buf);
                let original_buf = buf;
                let mut buf: &[u8] = decompressed;
                let (tx_type, new_buf) = T::from_compact(buf, flags.tx_type_len() as usize);
                buf = new_buf;
                let (success, new_buf) = bool::from_compact(buf, flags.success_len() as usize);
                buf = new_buf;
                let (cumulative_gas_used, new_buf) =
                    u64::from_compact(buf, flags.cumulative_gas_used_len() as usize);
                buf = new_buf;
                let (logs, _) = Vec::<Log>::from_compact(buf, buf.len());
                (Self { tx_type, success, cumulative_gas_used, logs }, original_buf)
            })
        } else {
            let (tx_type, new_buf) = T::from_compact(buf, flags.tx_type_len() as usize);
            buf = new_buf;
            let (success, new_buf) = bool::from_compact(buf, flags.success_len() as usize);
            buf = new_buf;
            let (cumulative_gas_used, new_buf) =
                u64::from_compact(buf, flags.cumulative_gas_used_len() as usize);
            buf = new_buf;
            let (logs, new_buf) = Vec::<Log>::from_compact(buf, buf.len());
            buf = new_buf;
            let obj = Self { tx_type, success, cumulative_gas_used, logs };
            (obj, buf)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::TxType;
    use proptest::proptest;
    use proptest_arbitrary_interop::arb;

    proptest! {
        #[test]
        fn roundtrip_receipt(receipt in arb::<AlloyEthereumReceipt<TxType>>()) {
            let mut compacted_receipt = Vec::<u8>::new();
            let len = receipt.to_compact(&mut compacted_receipt);
            let (decoded, _) = AlloyEthereumReceipt::<TxType>::from_compact(&compacted_receipt, len);
            assert_eq!(receipt, decoded)
        }
    }
}
