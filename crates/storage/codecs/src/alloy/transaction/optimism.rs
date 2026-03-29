//! Compact implementation for [`AlloyTxDeposit`]

use crate::{
    alloy::transaction::ethereum::{CompactEnvelope, Envelope, FromTxCompact, ToTxCompact},
    generate_tests,
    txtype::{
        COMPACT_EXTENDED_IDENTIFIER_FLAG, COMPACT_IDENTIFIER_EIP1559, COMPACT_IDENTIFIER_EIP2930,
        COMPACT_IDENTIFIER_LEGACY,
    },
    Compact,
};
use alloy_consensus::{
    constants::EIP7702_TX_TYPE_ID, Signed, TxEip1559, TxEip2930, TxEip7702, TxLegacy,
};
use alloy_primitives::{Address, Bytes, Sealed, Signature, TxKind, B256, U256};
use bytes::BufMut;
use op_alloy_consensus::{OpTxEnvelope, OpTxType, OpTypedTransaction, TxDeposit as AlloyTxDeposit};
use reth_codecs_derive::add_arbitrary_tests;

/// Compact storage helper for Mantle deposit transactions.
///
/// By deriving `Compact` here, any future changes or enhancements to the `Compact` derive
/// will automatically apply to this type.
///
/// # ⚠️ CRITICAL: On-disk binary compatibility — DO NOT change field types!
///
/// This struct's field types directly determine the Compact bitfield size (see
/// [`TxDeposit::bitflag_encoded_bytes()`]). Changing a field type changes the bitfield
/// layout, which makes ALL existing on-disk data **unreadable** (every field shifts by
/// N bytes). This was the root cause of the op-reth-rpc41 sync failure: `eth_value` was
/// accidentally changed from `Option<u128>` (1 bit) to `u128` (5 bits), growing the
/// bitfield from 2 to 3 bytes. See `TROUBLESHOOTING-OP-RETH-RPC41.md` §5.4 for details.
///
/// Specifically:
/// - `Option<T>` uses **1 bit** in the bitfield (None/Some flag only).
/// - `u128` uses **5 bits**, `u64` uses **4 bits**, `U256` uses **6 bits**.
/// - **Adding/removing `Option` wrapping changes the bit count and breaks the format.**
///
/// The field types here do NOT need to match `op_alloy_consensus::TxDeposit` exactly.
/// The `to_compact`/`from_compact` impl handles type conversion (e.g., `u128 ↔ Option<u128>`).
/// `mint` already uses this pattern: op-alloy has `u128`, Compact has `Option<u128>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Compact)]
#[cfg_attr(
    any(test, feature = "test-utils"),
    derive(arbitrary::Arbitrary, serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(feature = "test-utils", allow(unreachable_pub), visibility::make(pub))]
#[reth_codecs(crate = "crate")]
#[add_arbitrary_tests(crate, compact)]
pub(crate) struct TxDeposit {
    source_hash: B256,
    from: Address,
    to: TxKind,
    mint: Option<u128>,
    value: U256,
    gas_limit: u64,
    is_system_transaction: bool,
    /// DO NOT change to `u128` — see struct-level doc for why.
    /// Stored as `Option<u128>` (1 bit in bitfield) to preserve the 2-byte bitfield layout
    /// that all existing on-disk data uses. The op-alloy struct uses `u128`; the
    /// `to_compact`/`from_compact` impl converts via `0 → None` / `None → 0`.
    /// Guarded by `test_bitfield_size_must_be_2_bytes` test below.
    eth_value: Option<u128>,
    eth_tx_value: Option<u128>,
    input: Bytes,
}

impl Compact for AlloyTxDeposit {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let tx = TxDeposit {
            source_hash: self.source_hash,
            from: self.from,
            to: self.to,
            mint: match self.mint {
                0 => None,
                v => Some(v),
            },
            value: self.value,
            gas_limit: self.gas_limit,
            is_system_transaction: self.is_system_transaction,
            input: self.input.clone(),
            eth_value: match self.eth_value {
                0 => None,
                v => Some(v),
            },
            eth_tx_value: self.eth_tx_value,
        };
        tx.to_compact(buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        // Return the remaining slice from the inner from_compact to advance the cursor correctly.
        let (tx, remaining) = TxDeposit::from_compact(buf, len);
        let alloy_tx = Self {
            source_hash: tx.source_hash,
            from: tx.from,
            to: tx.to,
            mint: tx.mint.unwrap_or_default(),
            value: tx.value,
            gas_limit: tx.gas_limit,
            is_system_transaction: tx.is_system_transaction,
            input: tx.input,
            eth_value: tx.eth_value.unwrap_or_default(),
            eth_tx_value: tx.eth_tx_value,
        };
        (alloy_tx, remaining)
    }
}

impl crate::Compact for OpTxType {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        use crate::txtype::*;

        match self {
            Self::Legacy => COMPACT_IDENTIFIER_LEGACY,
            Self::Eip2930 => COMPACT_IDENTIFIER_EIP2930,
            Self::Eip1559 => COMPACT_IDENTIFIER_EIP1559,
            Self::Eip7702 => {
                buf.put_u8(EIP7702_TX_TYPE_ID);
                COMPACT_EXTENDED_IDENTIFIER_FLAG
            }
            Self::Deposit => {
                buf.put_u8(op_alloy_consensus::DEPOSIT_TX_TYPE_ID);
                COMPACT_EXTENDED_IDENTIFIER_FLAG
            }
        }
    }

    // For backwards compatibility purposes only 2 bits of the type are encoded in the identifier
    // parameter. In the case of a [`COMPACT_EXTENDED_IDENTIFIER_FLAG`], the full transaction type
    // is read from the buffer as a single byte.
    fn from_compact(mut buf: &[u8], identifier: usize) -> (Self, &[u8]) {
        use bytes::Buf;
        (
            match identifier {
                COMPACT_IDENTIFIER_LEGACY => Self::Legacy,
                COMPACT_IDENTIFIER_EIP2930 => Self::Eip2930,
                COMPACT_IDENTIFIER_EIP1559 => Self::Eip1559,
                COMPACT_EXTENDED_IDENTIFIER_FLAG => {
                    let extended_identifier = buf.get_u8();
                    match extended_identifier {
                        EIP7702_TX_TYPE_ID => Self::Eip7702,
                        op_alloy_consensus::DEPOSIT_TX_TYPE_ID => Self::Deposit,
                        _ => panic!("Unsupported OpTxType identifier: {extended_identifier}"),
                    }
                }
                _ => panic!("Unknown identifier for TxType: {identifier}"),
            },
            buf,
        )
    }
}

impl Compact for OpTypedTransaction {
    fn to_compact<B>(&self, out: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let identifier = self.tx_type().to_compact(out);
        match self {
            Self::Legacy(tx) => tx.to_compact(out),
            Self::Eip2930(tx) => tx.to_compact(out),
            Self::Eip1559(tx) => tx.to_compact(out),
            Self::Eip7702(tx) => tx.to_compact(out),
            Self::Deposit(tx) => tx.to_compact(out),
        };
        identifier
    }

    fn from_compact(buf: &[u8], identifier: usize) -> (Self, &[u8]) {
        let (tx_type, buf) = OpTxType::from_compact(buf, identifier);
        match tx_type {
            OpTxType::Legacy => {
                let (tx, buf) = Compact::from_compact(buf, buf.len());
                (Self::Legacy(tx), buf)
            }
            OpTxType::Eip2930 => {
                let (tx, buf) = Compact::from_compact(buf, buf.len());
                (Self::Eip2930(tx), buf)
            }
            OpTxType::Eip1559 => {
                let (tx, buf) = Compact::from_compact(buf, buf.len());
                (Self::Eip1559(tx), buf)
            }
            OpTxType::Eip7702 => {
                let (tx, buf) = Compact::from_compact(buf, buf.len());
                (Self::Eip7702(tx), buf)
            }
            OpTxType::Deposit => {
                let (tx, buf) = Compact::from_compact(buf, buf.len());
                (Self::Deposit(tx), buf)
            }
        }
    }
}

impl ToTxCompact for OpTxEnvelope {
    fn to_tx_compact(&self, buf: &mut (impl BufMut + AsMut<[u8]>)) {
        match self {
            Self::Legacy(tx) => tx.tx().to_compact(buf),
            Self::Eip2930(tx) => tx.tx().to_compact(buf),
            Self::Eip1559(tx) => tx.tx().to_compact(buf),
            Self::Eip7702(tx) => tx.tx().to_compact(buf),
            Self::Deposit(tx) => tx.to_compact(buf),
        };
    }
}

impl FromTxCompact for OpTxEnvelope {
    type TxType = OpTxType;

    fn from_tx_compact(buf: &[u8], tx_type: OpTxType, signature: Signature) -> (Self, &[u8]) {
        match tx_type {
            OpTxType::Legacy => {
                let (tx, buf) = TxLegacy::from_compact(buf, buf.len());
                let tx = Signed::new_unhashed(tx, signature);
                (Self::Legacy(tx), buf)
            }
            OpTxType::Eip2930 => {
                let (tx, buf) = TxEip2930::from_compact(buf, buf.len());
                let tx = Signed::new_unhashed(tx, signature);
                (Self::Eip2930(tx), buf)
            }
            OpTxType::Eip1559 => {
                let (tx, buf) = TxEip1559::from_compact(buf, buf.len());
                let tx = Signed::new_unhashed(tx, signature);
                (Self::Eip1559(tx), buf)
            }
            OpTxType::Eip7702 => {
                let (tx, buf) = TxEip7702::from_compact(buf, buf.len());
                let tx = Signed::new_unhashed(tx, signature);
                (Self::Eip7702(tx), buf)
            }
            OpTxType::Deposit => {
                let (tx, buf) = op_alloy_consensus::TxDeposit::from_compact(buf, buf.len());
                let tx = Sealed::new(tx);
                (Self::Deposit(tx), buf)
            }
        }
    }
}

const DEPOSIT_SIGNATURE: Signature = Signature::new(U256::ZERO, U256::ZERO, false);

impl Envelope for OpTxEnvelope {
    fn signature(&self) -> &Signature {
        match self {
            Self::Legacy(tx) => tx.signature(),
            Self::Eip2930(tx) => tx.signature(),
            Self::Eip1559(tx) => tx.signature(),
            Self::Eip7702(tx) => tx.signature(),
            Self::Deposit(_) => &DEPOSIT_SIGNATURE,
        }
    }

    fn tx_type(&self) -> Self::TxType {
        Self::tx_type(self)
    }
}

impl Compact for OpTxEnvelope {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: BufMut + AsMut<[u8]>,
    {
        CompactEnvelope::to_compact(self, buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        CompactEnvelope::from_compact(buf, len)
    }
}

generate_tests!(#[crate, compact] OpTypedTransaction, OpTypedTransactionTests);

#[cfg(test)]
mod mantle_compact_roundtrip_tests {
    use super::*;
    use alloy_primitives::{address, hex, Bytes, TxKind, B256, U256};
    use op_alloy_consensus::TxDeposit as AlloyTxDeposit;

    /// Simulate a typical Mantle mainnet deposit tx (Limb era):
    /// `eth_value=0`, `eth_tx_value=None`, input=260 bytes (Bedrock `setL1BlockValues`)
    ///
    /// This is the exact scenario that causes the 243-byte bug on op-reth-rpc41.
    #[test]
    fn test_compact_roundtrip_mantle_deposit_tx_bedrock_260b() {
        // Bedrock setL1BlockValues calldata (260 bytes): selector 015d8eb9 + 8 * 32-byte args
        let mut input_data = vec![0x01, 0x5d, 0x8e, 0xb9]; // selector
                                                           // 8 ABI-encoded uint256 args (8 * 32 = 256 bytes)
        for i in 0u8..8 {
            let mut arg = [0u8; 32];
            arg[31] = i + 1; // non-zero last byte for each arg
            input_data.extend_from_slice(&arg);
        }
        assert_eq!(input_data.len(), 260, "Bedrock calldata must be 260 bytes");

        let original = AlloyTxDeposit {
            source_hash: B256::from(hex!(
                "520df4f6f1f883397e640e1f837e3d29b119241a4fb50ff483256d850562f903"
            )),
            from: address!("deaddeaddeaddeaddeaddeaddeaddeaddead0001"),
            to: TxKind::Call(address!("4200000000000000000000000000000000000015")),
            mint: 0,
            value: U256::ZERO,
            gas_limit: 1_000_000,
            is_system_transaction: false,
            eth_value: 0,
            input: Bytes::from(input_data),
            eth_tx_value: None,
        };

        // Compact roundtrip — use Vec::new() so buffer grows to exact encoded size.
        // Using a pre-allocated buffer (e.g., vec![0u8; 4096]) would cause the Bytes
        // field to read trailing zeros, since it consumes the remaining buffer.
        let mut compact_buf = Vec::new();
        let _len = original.to_compact(&mut compact_buf);
        let (restored, _remaining) = AlloyTxDeposit::from_compact(&compact_buf, compact_buf.len());

        // Field-level comparison
        assert_eq!(original.source_hash, restored.source_hash, "source_hash mismatch");
        assert_eq!(original.from, restored.from, "from mismatch");
        assert_eq!(original.to, restored.to, "to mismatch");
        assert_eq!(original.mint, restored.mint, "mint mismatch");
        assert_eq!(original.value, restored.value, "value mismatch");
        assert_eq!(original.gas_limit, restored.gas_limit, "gas_limit mismatch");
        assert_eq!(
            original.is_system_transaction, restored.is_system_transaction,
            "is_system_transaction mismatch"
        );
        assert_eq!(original.eth_value, restored.eth_value, "eth_value mismatch");
        assert_eq!(original.eth_tx_value, restored.eth_tx_value, "eth_tx_value mismatch");
        assert_eq!(
            original.input.len(),
            restored.input.len(),
            "input LENGTH mismatch: expected {} got {}",
            original.input.len(),
            restored.input.len()
        );
        assert_eq!(original.input, restored.input, "input CONTENT mismatch");
    }

    /// Test with `eth_value=200`, `eth_tx_value=Some(300)` — non-zero Mantle extension fields
    #[test]
    fn test_compact_roundtrip_mantle_deposit_tx_nonzero_fields() {
        let original = AlloyTxDeposit {
            source_hash: B256::with_last_byte(42),
            from: address!("deaddeaddeaddeaddeaddeaddeaddeaddead0001"),
            to: TxKind::Call(address!("4200000000000000000000000000000000000015")),
            mint: 1000,
            value: U256::from(5000),
            gas_limit: 1_000_000,
            is_system_transaction: false,
            eth_value: 200,
            input: Bytes::from_static(&[0x01, 0x5d, 0x8e, 0xb9, 0xaa, 0xbb, 0xcc]),
            eth_tx_value: Some(300),
        };

        let mut compact_buf = Vec::new();
        let _len = original.to_compact(&mut compact_buf);
        let (restored, _) = AlloyTxDeposit::from_compact(&compact_buf, compact_buf.len());

        assert_eq!(original.eth_value, restored.eth_value, "eth_value mismatch");
        assert_eq!(original.eth_tx_value, restored.eth_tx_value, "eth_tx_value mismatch");
        assert_eq!(original.input, restored.input, "input mismatch");
        assert_eq!(original, restored, "full tx mismatch");
    }

    /// Test `eth_tx_value=Some(0)` — edge case where Option is Some but value is zero
    #[test]
    fn test_compact_roundtrip_eth_tx_value_some_zero() {
        let original = AlloyTxDeposit {
            source_hash: B256::with_last_byte(1),
            from: address!("deaddeaddeaddeaddeaddeaddeaddeaddead0001"),
            to: TxKind::Call(address!("4200000000000000000000000000000000000015")),
            mint: 0,
            value: U256::ZERO,
            gas_limit: 1_000_000,
            is_system_transaction: false,
            eth_value: 0,
            input: Bytes::from_static(&[0x01, 0x02, 0x03]),
            eth_tx_value: Some(0),
        };

        let mut compact_buf = Vec::new();
        let _len = original.to_compact(&mut compact_buf);
        let (restored, _) = AlloyTxDeposit::from_compact(&compact_buf, compact_buf.len());

        assert_eq!(original.eth_value, restored.eth_value, "eth_value mismatch");
        assert_eq!(
            original.eth_tx_value, restored.eth_tx_value,
            "eth_tx_value mismatch: expected {:?} got {:?}",
            original.eth_tx_value, restored.eth_tx_value
        );
        assert_eq!(
            original.input,
            restored.input,
            "input mismatch: expected {} bytes got {} bytes",
            original.input.len(),
            restored.input.len()
        );
    }

    /// CRITICAL GUARD TEST: The Compact bitfield for `TxDeposit` MUST be exactly 2 bytes.
    ///
    /// If this test fails, you have changed a field type in the `TxDeposit` struct in a way
    /// that changes the bitfield size. This WILL break all existing on-disk data for every
    /// Mantle node that upgrades. DO NOT simply update the expected value — read the
    /// struct-level doc comment on `TxDeposit` and `TROUBLESHOOTING-OP-RETH-RPC41.md` §5.4.
    ///
    /// Background: Commit `563f0492b2` changed `eth_value: Option<u128>` (1 bit) to
    /// `eth_value: u128` (5 bits), growing the bitfield from 2→3 bytes. This caused every
    /// deposit tx read from DB to have all fields shifted by 1 byte, producing corrupt data
    /// (e.g., input truncated from 260→243 bytes). The fix was to revert to `Option<u128>`.
    #[test]
    fn test_bitfield_size_must_be_2_bytes() {
        assert_eq!(
            TxDeposit::bitflag_encoded_bytes(),
            2,
            "TxDeposit bitfield size changed! This breaks ALL existing on-disk data. \
             See struct-level doc on TxDeposit and TROUBLESHOOTING-OP-RETH-RPC41.md §5.4. \
             Current field bit counts: B256=0, Address=0, TxKind=1, Option<u128>=1, \
             U256=6, u64=4, bool=1, Option<u128>=1, Option<u128>=1, Bytes=0 → 15 bits → 2 bytes."
        );
    }

    /// Roundtrip test using real Mantle mainnet deposit tx bytes from block 87,910,504.
    ///
    /// These bytes were fetched from geth via `debug_getRawTransaction` and represent a
    /// typical Limb-era L1 attributes deposit tx with 260-byte input calldata. This is the
    /// exact tx pattern that triggered the op-reth-rpc41 "data is unexpected length: 243" bug.
    ///
    /// The test decodes the real RLP bytes → Compact encode → Compact decode → RLP re-encode,
    /// and asserts the output is byte-identical to the original.
    #[test]
    fn test_compact_roundtrip_real_mainnet_block_87910504() {
        use alloy_eips::eip2718::{Decodable2718, Encodable2718};

        // Real deposit tx from Mantle mainnet block 87,910,504 (353 bytes EIP-2718 envelope).
        // Fetched via: cast rpc debug_getRawTransaction <txhash> --rpc-url <geth>
        // Type 0x7e (126) = deposit tx.
        let raw_bytes = hex!(
            "7ef9015aa0f129853cf1f38fe1fbcf264f82d80e8fd4532bba9213ff0b0846890cbd2f1656"
            "94deaddeaddeaddeaddeaddeaddeaddeaddead0001944200000000000000000000000000000000"
            "000015808083"
            "0f42408080b90104015d8eb900000000000000000000000000000000000000000000000000"
            "000000000f424000000000000000000000000000000000000000000000000000000000676f0775"
            "000000000000000000000000000000000000000000000000000000003b9aca0000000000000000"
            "0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000027100000000000000000000000003c44cdddb6a900fa2b585dd299e03d12fa4293bc00000000000000000000000000000000000000000000000000000000000000012710"
        );
        assert_eq!(raw_bytes.len(), 353, "Real mainnet tx must be 353 bytes (EIP-2718 envelope)");

        // Step 1: Decode RLP (simulates what reth does when receiving from Engine API)
        let tx = AlloyTxDeposit::decode_2718(&mut &raw_bytes[..])
            .expect("Failed to decode real mainnet deposit tx RLP");

        // Verify key fields from the real tx
        assert_eq!(tx.from, address!("deaddeaddeaddeaddeaddeaddeaddeaddead0001"));
        assert_eq!(tx.to, TxKind::Call(address!("4200000000000000000000000000000000000015")));
        assert_eq!(tx.gas_limit, 1_000_000);
        assert_eq!(tx.eth_value, 0);
        assert_eq!(tx.input.len(), 260, "Bedrock setL1BlockValues calldata = 260 bytes");

        // Step 2: Compact encode (simulates DB write)
        let mut compact_buf = Vec::new();
        let _len = tx.to_compact(&mut compact_buf);

        // Step 3: Compact decode (simulates DB read)
        let (restored, _) = AlloyTxDeposit::from_compact(&compact_buf, compact_buf.len());

        // Step 4: Verify field-level correctness (catches 1-byte shift bugs)
        assert_eq!(tx.source_hash, restored.source_hash, "source_hash mismatch");
        assert_eq!(tx.from, restored.from, "from mismatch");
        assert_eq!(tx.to, restored.to, "to mismatch");
        assert_eq!(tx.mint, restored.mint, "mint mismatch");
        assert_eq!(tx.value, restored.value, "value mismatch");
        assert_eq!(tx.gas_limit, restored.gas_limit, "gas_limit mismatch");
        assert_eq!(
            tx.is_system_transaction, restored.is_system_transaction,
            "is_system_transaction mismatch"
        );
        assert_eq!(tx.eth_value, restored.eth_value, "eth_value mismatch");
        assert_eq!(tx.eth_tx_value, restored.eth_tx_value, "eth_tx_value mismatch");
        assert_eq!(
            tx.input.len(),
            restored.input.len(),
            "input LENGTH mismatch: expected {} got {} (this is the exact rpc41 bug symptom!)",
            tx.input.len(),
            restored.input.len()
        );
        assert_eq!(tx.input, restored.input, "input CONTENT mismatch");

        // Step 5: RLP re-encode and compare against the decoded tx's own RLP encoding.
        // We compare against tx.encoded_2718() (not raw_bytes) to isolate the Compact
        // roundtrip from any RLP decode/re-encode differences.
        let rlp_original = tx.encoded_2718();
        let rlp_restored = restored.encoded_2718();
        assert_eq!(
            rlp_original.len(),
            rlp_restored.len(),
            "RLP length mismatch: original {}B vs restored {}B",
            rlp_original.len(),
            rlp_restored.len()
        );
        assert_eq!(
            rlp_original,
            rlp_restored,
            "RLP bytes differ after Compact roundtrip! This means the Compact codec is corrupting data."
        );
    }

    /// Full RLP → Compact → RLP roundtrip test.
    /// This simulates the exact reth pipeline: Engine API receive → DB store → DB read → serve.
    #[test]
    fn test_full_rlp_compact_rlp_roundtrip() {
        use alloy_eips::eip2718::Encodable2718;

        // Build a realistic Mantle deposit tx
        let mut input_data = vec![0x01, 0x5d, 0x8e, 0xb9]; // Bedrock selector
        for i in 0u8..8 {
            let mut arg = [0u8; 32];
            arg[31] = i + 1;
            input_data.extend_from_slice(&arg);
        }

        let original = AlloyTxDeposit {
            source_hash: B256::from(hex!(
                "520df4f6f1f883397e640e1f837e3d29b119241a4fb50ff483256d850562f903"
            )),
            from: address!("deaddeaddeaddeaddeaddeaddeaddeaddead0001"),
            to: TxKind::Call(address!("4200000000000000000000000000000000000015")),
            mint: 0,
            value: U256::ZERO,
            gas_limit: 1_000_000,
            is_system_transaction: false,
            eth_value: 0,
            input: Bytes::from(input_data),
            eth_tx_value: None,
        };

        // Step 1: RLP encode (simulate what geth/sequencer produces)
        let rlp_original = original.encoded_2718();

        // Step 2: Compact encode (simulate DB write)
        let mut compact_buf = Vec::new();
        let _len = original.to_compact(&mut compact_buf);

        // Step 3: Compact decode (simulate DB read)
        let (restored, _) = AlloyTxDeposit::from_compact(&compact_buf, compact_buf.len());

        // Step 4: RLP re-encode (simulate serving via Engine API / RPC)
        let rlp_restored = restored.encoded_2718();

        // Compare: the full pipeline must produce identical bytes
        assert_eq!(
            rlp_original.len(),
            rlp_restored.len(),
            "RLP length mismatch after Compact roundtrip: original {}B vs restored {}B",
            rlp_original.len(),
            rlp_restored.len()
        );
        assert_eq!(
            rlp_original,
            rlp_restored,
            "RLP bytes differ after Compact roundtrip!\noriginal: {}\nrestored: {}",
            hex::encode(&rlp_original),
            hex::encode(&rlp_restored)
        );
    }
}
