//! Conversions from flashblock types to Engine API payload types.
//!
//! This module provides the canonical implementations for converting flashblock data structures
//! into the various versions of execution payloads required by the Engine API.
//!
//! TODO: ideally we move this to op-alloy-rpc-types-engine where `from_block_unchecked` is defined
//! and define a similar called `from_flashblock_sequence_unchecked`.
//! The goal is to have the conversion logic in one place to avoid missing steps.

use crate::{
    ExecutionPayloadBaseV1, ExecutionPayloadFlashblockDeltaV1, FlashBlockCompleteSequence,
};
use alloy_rpc_types_engine::{ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3};
use op_alloy_rpc_types_engine::{OpExecutionData, OpExecutionPayloadV4};

/// Converts flashblock base and delta into `ExecutionPayloadV1`.
///
/// This is the foundational conversion that constructs the base execution payload
/// from the static base fields and the accumulated delta state.
fn to_execution_payload_v1(
    base: &ExecutionPayloadBaseV1,
    delta: &ExecutionPayloadFlashblockDeltaV1,
) -> ExecutionPayloadV1 {
    ExecutionPayloadV1 {
        parent_hash: base.parent_hash,
        fee_recipient: base.fee_recipient,
        state_root: delta.state_root,
        receipts_root: delta.receipts_root,
        logs_bloom: delta.logs_bloom,
        prev_randao: base.prev_randao,
        block_number: base.block_number,
        gas_limit: base.gas_limit,
        gas_used: delta.gas_used,
        timestamp: base.timestamp,
        extra_data: base.extra_data.clone(),
        base_fee_per_gas: base.base_fee_per_gas,
        block_hash: delta.block_hash,
        transactions: delta.transactions.clone(),
    }
}

/// Converts flashblock base and delta into `ExecutionPayloadV2`.
///
/// Wraps V1 payload with withdrawals from the flashblock delta.
fn to_execution_payload_v2(
    base: &ExecutionPayloadBaseV1,
    delta: &ExecutionPayloadFlashblockDeltaV1,
) -> ExecutionPayloadV2 {
    ExecutionPayloadV2 {
        payload_inner: to_execution_payload_v1(base, delta),
        withdrawals: delta.withdrawals.clone(),
    }
}

/// Converts flashblock base and delta into `ExecutionPayloadV3`.
///
/// Wraps V2 payload with blob gas fields set to zero, as flashblocks
/// currently don't track blob gas usage.
fn to_execution_payload_v3(
    base: &ExecutionPayloadBaseV1,
    delta: &ExecutionPayloadFlashblockDeltaV1,
) -> ExecutionPayloadV3 {
    ExecutionPayloadV3 {
        payload_inner: to_execution_payload_v2(base, delta),
        // TODO: update when ExecutionPayloadFlashblockDeltaV1 contains `blob_gas_used` after Jovian
        blob_gas_used: 0,
        excess_blob_gas: 0,
    }
}

/// Converts flashblock base and delta into `OpExecutionPayloadV4`.
///
/// Wraps V3 payload with the withdrawals root from the flashblock delta.
fn to_execution_payload_v4(
    base: &ExecutionPayloadBaseV1,
    delta: &ExecutionPayloadFlashblockDeltaV1,
) -> OpExecutionPayloadV4 {
    OpExecutionPayloadV4 {
        payload_inner: to_execution_payload_v3(base, delta),
        withdrawals_root: delta.withdrawals_root,
    }
}

/// Converts a complete flashblock sequence into `OpExecutionData`.
///
/// This is the primary conversion used when submitting a completed flashblock sequence
/// to the Engine API via `engine_newPayloadV4`. It combines the base payload fields
/// from the first flashblock with the accumulated state from the last flashblock.
impl From<&FlashBlockCompleteSequence> for OpExecutionData {
    fn from(sequence: &FlashBlockCompleteSequence) -> Self {
        let base = sequence.payload_base();
        let last = sequence.last();

        // Build the V4 payload using the conversion functions
        let payload_v4 = to_execution_payload_v4(base, &last.diff);

        // Use OpExecutionData::v4 helper to construct the complete data with sidecar
        Self::v4(payload_v4, vec![], base.parent_beacon_block_root, Default::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{ruint::aliases::U256, Address, Bloom, Bytes, B256};

    fn sample_base() -> ExecutionPayloadBaseV1 {
        ExecutionPayloadBaseV1 {
            parent_beacon_block_root: B256::random(),
            parent_hash: B256::random(),
            fee_recipient: Address::random(),
            prev_randao: B256::random(),
            block_number: 100,
            gas_limit: 30_000_000,
            timestamp: 1234567890,
            extra_data: Bytes::from(vec![1, 2, 3]),
            base_fee_per_gas: U256::random(),
        }
    }

    fn sample_delta() -> ExecutionPayloadFlashblockDeltaV1 {
        ExecutionPayloadFlashblockDeltaV1 {
            state_root: B256::random(),
            receipts_root: B256::random(),
            logs_bloom: Bloom::default(),
            gas_used: 21_000,
            block_hash: B256::random(),
            transactions: vec![],
            withdrawals: vec![],
            withdrawals_root: B256::random(),
        }
    }

    #[test]
    fn test_v1_conversion() {
        let base = sample_base();
        let delta = sample_delta();

        let payload = to_execution_payload_v1(&base, &delta);

        assert_eq!(payload.parent_hash, base.parent_hash);
        assert_eq!(payload.fee_recipient, base.fee_recipient);
        assert_eq!(payload.state_root, delta.state_root);
        assert_eq!(payload.receipts_root, delta.receipts_root);
        assert_eq!(payload.gas_used, delta.gas_used);
        assert_eq!(payload.block_hash, delta.block_hash);
    }

    #[test]
    fn test_v2_conversion() {
        let base = sample_base();
        let delta = sample_delta();

        let payload = to_execution_payload_v2(&base, &delta);

        assert_eq!(payload.payload_inner.parent_hash, base.parent_hash);
        assert!(payload.withdrawals.is_empty());
    }

    #[test]
    fn test_v3_conversion() {
        let base = sample_base();
        let delta = sample_delta();

        let payload = to_execution_payload_v3(&base, &delta);

        assert_eq!(payload.payload_inner.payload_inner.parent_hash, base.parent_hash);
        assert_eq!(payload.blob_gas_used, 0);
        assert_eq!(payload.excess_blob_gas, 0);
    }

    #[test]
    fn test_v4_conversion() {
        let base = sample_base();
        let delta = sample_delta();

        let payload = to_execution_payload_v4(&base, &delta);

        assert_eq!(payload.payload_inner.payload_inner.payload_inner.parent_hash, base.parent_hash);
        assert_eq!(payload.withdrawals_root, delta.withdrawals_root);
    }

    // <https://unichain-sepolia.blockscout.com/block/35535698>
    #[test]
    fn convert_unichain_payload() {
        let raw_sequence = r#"{"inner":[{"payload_id":"0x03c446f063e3735a","index":0,"base":{"parent_beacon_block_root":"0xf6d335a6b2b4fd8fb539cd51a49769df4d53c31a90c54dd270e54542638ff101","parent_hash":"0x06ff95a9cd23b0328da74a984aa986b2e01d377dab1825f1029e39ece6c4a3ea","fee_recipient":"0x4200000000000000000000000000000000000011","prev_randao":"0x8beee738d20a9d77c5f27e9cb799ebe5b536f0985efad5f7d77ebff47f092c4a","block_number":"0x21e3b52","gas_limit":"0x3938700","timestamp":"0x690be89e","extra_data":"0x00000000320000000c","base_fee_per_gas":"0x33"},"diff":{"state_root":"0xb29a9bcae8cf3ae6d68985fcd70db80b3818cd629c9d5da0bb116451739b2078","receipts_root":"0x91d8ad10740ccfc1bd848fba0e02668d95769c08eeea30f10698692ba86c6159","logs_bloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","gas_used":"0x10994","block_hash":"0xa66f8562a861f906a2438d7d6ba79495640d98d9c6922b9605c54b57f97a345c","transactions":["0x7ef90104a035dd2ec802504a143048c7830f8f570e0d6cf5147217af869939c6b4ba710a3694deaddeaddeaddeaddeaddeaddeaddeaddead00019442000000000000000000000000000000000000158080830f424080b8b0098999be000007d0000dbba0000000000000000800000000690be848000000000092042e000000000000000000000000000000000000000000000000000000000000000900000000000000000000000000000000000000000000000000000000000000010ffd7e2fb2c36e5f27c015872ce733a7b4f3fc0f4ee668d7469c557c48f8250f0000000000000000000000004ab3387810ef500bfe05a49dc53a44c222cbab3e000000000000000000000000","0x02f87e8205158401c8ea9180338255789400000000000000000000000000000000000000008096426c6f636b204e756d6265723a203335353335363938c080a091f83058c881d9ad71c179ce680326501702eb68150d20b2bf7786e388f954a2a0180185d83e503f11bf3c265c1f9296ed8d3d7c04031cd8bb30509ad188ce7bbc"],"withdrawals":[],"withdrawals_root":"0x62ed62e0391b081bf172f287fbbe75e87d8a6c22f1d3b1f1aef4788c134633d2"},"metadata":{"block_number":35535698,"new_account_balances":{"0x0000f90827f1c53a10cb7a02335b175320002935":"0x0","0x000f3df6d732807ef1319fb7b8bb8522d0beac02":"0x0","0x4200000000000000000000000000000000000015":"0x0","0x4200000000000000000000000000000000000019":"0x3027604f4611c8dfdb","0x420000000000000000000000000000000000001a":"0xc9cd03d2dd193f92","0xc0043c50ba044a5f948d32faed806913d781d428":"0xabdabacc8372acf2","0xdeaddeaddeaddeaddeaddeaddeaddeaddead0001":"0x47f9ed8a5238da96"},"receipts":{"0x613834aaeeb676fb45e312531c0cac40485a2c1b7b8ad8d648e5034dddfb5f03":{"Deposit":{"status":"0x1","cumulativeGasUsed":"0xb41c","logs":[],"depositNonce":"0x21e3b52","depositReceiptVersion":"0x1"}},"0x89f7852587dca7dd2f31b02f3a93f47669a82784025d607ec7c336b66af84a19":{"Eip1559":{"status":"0x1","cumulativeGasUsed":"0x10994","logs":[]}}}}},{"payload_id":"0x03c446f063e3735a","index":1,"base":null,"diff":{"state_root":"0xfb1794f74d405b345672c57a5053c6105cc55c8e63f96fb0db5b0260df42413a","receipts_root":"0x1eaaaeb9d43bead7d32b90f1b320589174c63d2fa8f5fd366f841a205b1eb2e0","logs_bloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002000000001000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000040000000000000000000000000000000000000000000000000004040000000000000000000000000000000000000000000000000000000000000000000000","gas_used":"0x18f7d","block_hash":"0x67b0521ebfcb03d6ce2b6e1bad9c9c66795365f63ad8dc51e1e8f582a5ab7821","transactions":["0x02f86c8205158401c8ea92803382880994f878f0340bf132c28f3211e8b46c569edf81749580843fd553e8c001a0d73ce313aafea312e0b7244767e45f8b05d50305e0f4e4c3c564ddc751666815a02ee015ce2363311823c0b2e96bfb0e8090fd53c6cdd99be8cf343af123036dfc"],"withdrawals":[],"withdrawals_root":"0x62ed62e0391b081bf172f287fbbe75e87d8a6c22f1d3b1f1aef4788c134633d2"},"metadata":{"block_number":35535698,"new_account_balances":{"0x0000f90827f1c53a10cb7a02335b175320002935":"0x0","0x000f3df6d732807ef1319fb7b8bb8522d0beac02":"0x0","0x4200000000000000000000000000000000000015":"0x0","0x4200000000000000000000000000000000000019":"0x3027604f4611e38d46","0x420000000000000000000000000000000000001a":"0xc9cd03d2dd194008","0xc0043c50ba044a5f948d32faed806913d781d428":"0xabdabacc8357ff11","0xdeaddeaddeaddeaddeaddeaddeaddeaddead0001":"0x47f9ed8a5238da96","0xf878f0340bf132c28f3211e8b46c569edf817495":"0x0"},"receipts":{"0x8416aa3bb024ea5dce8fc00d21a500df4e782743bdf245589b5aba920324df11":{"Eip1559":{"status":"0x1","cumulativeGasUsed":"0x18f7d","logs":[{"address":"0xf878f0340bf132c28f3211e8b46c569edf817495","topics":["0xfaddd0b06e793a583e92393ad3f98637e560462ee98db1f2888f141124ee64ca"],"data":"0x000000000000000000000000000000000000000000000000000000000029ac5c"}]}}}}},{"payload_id":"0x03c446f063e3735a","index":2,"base":null,"diff":{"state_root":"0x90dd105c4a2a0dd9ffe994204bfa3e2b4f70f7ea760d5cb9a4263f26a89f91b4","receipts_root":"0x0fff0488aa3732c34018b938839ab2f0caa96018221e4ffaeca011fb06ba288f","logs_bloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002000000001000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000040000000000000000000000000000000000000000000000000004040000000000000000000000000000000000000000000000000000000000000000000000","gas_used":"0x21566","block_hash":"0x720feb7457110a565b479fafbaa89cc984f5d673846a27d44bbb8cf5200b32fe","transactions":["0x02f86c8205158401c8ea93803382880994f878f0340bf132c28f3211e8b46c569edf81749580843fd553e8c001a0f8cd94080642e116bc772f36a02d002505227aa542e1c13e5129ab40b8b037fba00608318d3895388e39b218bcb275380cebc566e68f26d3d434e32b8b58366cdf"],"withdrawals":[],"withdrawals_root":"0x62ed62e0391b081bf172f287fbbe75e87d8a6c22f1d3b1f1aef4788c134633d2"},"metadata":{"block_number":35535698,"new_account_balances":{"0x0000f90827f1c53a10cb7a02335b175320002935":"0x0","0x000f3df6d732807ef1319fb7b8bb8522d0beac02":"0x0","0x4200000000000000000000000000000000000015":"0x0","0x4200000000000000000000000000000000000019":"0x3027604f4611fe3ab1","0x420000000000000000000000000000000000001a":"0xc9cd03d2dd19407e","0xc0043c50ba044a5f948d32faed806913d781d428":"0xabdabacc833d5130","0xdeaddeaddeaddeaddeaddeaddeaddeaddead0001":"0x47f9ed8a5238da96","0xf878f0340bf132c28f3211e8b46c569edf817495":"0x0"},"receipts":{"0x9afeb04e8858ec2c17d1f6543fc3c00772ef05a7177733770c186cee02c67112":{"Eip1559":{"status":"0x1","cumulativeGasUsed":"0x21566","logs":[{"address":"0xf878f0340bf132c28f3211e8b46c569edf817495","topics":["0xfaddd0b06e793a583e92393ad3f98637e560462ee98db1f2888f141124ee64ca"],"data":"0x000000000000000000000000000000000000000000000000000000000029ac5d"}]}}}}},{"payload_id":"0x03c446f063e3735a","index":3,"base":null,"diff":{"state_root":"0x71f8c60fdfdd84cffda3b0b6af7c8ff92195918f4fc2abae750a7306521ac0dc","receipts_root":"0xa62d1d98f56ffb1464a2beb185484253df68208004306e155c0bd1519137afe6","logs_bloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002000000001000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000040000000000000000000000000000000000000000000000000004040000000000000000000000000000000000000000000000000000000000000000000000","gas_used":"0x29b4f","block_hash":"0x670844e30f7325d4f290ea375e01f7e819afca317fc7db9723e6867a184984fa","transactions":["0x02f86c8205158401c8ea94803382880994f878f0340bf132c28f3211e8b46c569edf81749580843fd553e8c080a04368492ec1d087703aaf6f5fefe4427b3bf382e5cd07133f638bb6701f15fe61a05e28757fbdc7e744118be36d5a1548eb7c009eefcb5dc5c5040e09c2fc6de9d8"],"withdrawals":[],"withdrawals_root":"0x62ed62e0391b081bf172f287fbbe75e87d8a6c22f1d3b1f1aef4788c134633d2"},"metadata":{"block_number":35535698,"new_account_balances":{"0x0000f90827f1c53a10cb7a02335b175320002935":"0x0","0x000f3df6d732807ef1319fb7b8bb8522d0beac02":"0x0","0x4200000000000000000000000000000000000015":"0x0","0x4200000000000000000000000000000000000019":"0x3027604f461218e81c","0x420000000000000000000000000000000000001a":"0xc9cd03d2dd1940f4","0xc0043c50ba044a5f948d32faed806913d781d428":"0xabdabacc8322a34f","0xdeaddeaddeaddeaddeaddeaddeaddeaddead0001":"0x47f9ed8a5238da96","0xf878f0340bf132c28f3211e8b46c569edf817495":"0x0"},"receipts":{"0x68e4cb3cafd045680f66411af5c22bd4f239972551c668fde34fb495a07c06e7":{"Eip1559":{"status":"0x1","cumulativeGasUsed":"0x29b4f","logs":[{"address":"0xf878f0340bf132c28f3211e8b46c569edf817495","topics":["0xfaddd0b06e793a583e92393ad3f98637e560462ee98db1f2888f141124ee64ca"],"data":"0x000000000000000000000000000000000000000000000000000000000029ac5e"}]}}}}},{"payload_id":"0x03c446f063e3735a","index":4,"base":null,"diff":{"state_root":"0x5615e4342d231c352438f0ba6a8f0f641459f67961961764b781a909969b28ad","receipts_root":"0x588e1d47b0618d7e935b20c3945cba3b7b8c00141904f79ceed20312ea502e63","logs_bloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002000000001000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000040000000000000000000000000000000000000000000000000004040000000000000000000000000000000000000000000000000000000000000000000000","gas_used":"0x32138","block_hash":"0xc463a3120c35268f610d969f5608b479332ef10953af77c7a6be806195831196","transactions":["0x02f86c8205158401c8ea95803382880994f878f0340bf132c28f3211e8b46c569edf81749580843fd553e8c080a0802ba6d4f37e3b8de96095bd0b216144f276171d16dc62a004f1a89009af5deea00f0c6250cfd1a062a1bc2bc353a5c227a980cac0f233b7be8932f2192342ec4f"],"withdrawals":[],"withdrawals_root":"0x62ed62e0391b081bf172f287fbbe75e87d8a6c22f1d3b1f1aef4788c134633d2"},"metadata":{"block_number":35535698,"new_account_balances":{"0x0000f90827f1c53a10cb7a02335b175320002935":"0x0","0x000f3df6d732807ef1319fb7b8bb8522d0beac02":"0x0","0x4200000000000000000000000000000000000015":"0x0","0x4200000000000000000000000000000000000019":"0x3027604f4612339587","0x420000000000000000000000000000000000001a":"0xc9cd03d2dd19416a","0xc0043c50ba044a5f948d32faed806913d781d428":"0xabdabacc8307f56e","0xdeaddeaddeaddeaddeaddeaddeaddeaddead0001":"0x47f9ed8a5238da96","0xf878f0340bf132c28f3211e8b46c569edf817495":"0x0"},"receipts":{"0x6d65b11050eee84c3f492106ca95057825d34274b5ca9bcf533885a7ff238d39":{"Eip1559":{"status":"0x1","cumulativeGasUsed":"0x32138","logs":[{"address":"0xf878f0340bf132c28f3211e8b46c569edf817495","topics":["0xfaddd0b06e793a583e92393ad3f98637e560462ee98db1f2888f141124ee64ca"],"data":"0x000000000000000000000000000000000000000000000000000000000029ac5f"}]}}}}}],"state_root":null}
"#;

        let raw_block = r#"{"baseFeePerGas":"0x33","blobGasUsed":"0x0","difficulty":"0x0","excessBlobGas":"0x0","extraData":"0x00000000320000000c","gasLimit":"0x3938700","gasUsed":"0x32138","hash":"0xc463a3120c35268f610d969f5608b479332ef10953af77c7a6be806195831196","logsBloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002000000001000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000040000000000000000000000000000000000000000000000000004040000000000000000000000000000000000000000000000000000000000000000000000","miner":"0x4200000000000000000000000000000000000011","mixHash":"0x8beee738d20a9d77c5f27e9cb799ebe5b536f0985efad5f7d77ebff47f092c4a","nonce":"0x0000000000000000","number":"0x21e3b52","parentBeaconBlockRoot":"0xf6d335a6b2b4fd8fb539cd51a49769df4d53c31a90c54dd270e54542638ff101","parentHash":"0x06ff95a9cd23b0328da74a984aa986b2e01d377dab1825f1029e39ece6c4a3ea","receiptsRoot":"0x588e1d47b0618d7e935b20c3945cba3b7b8c00141904f79ceed20312ea502e63","requestsHash":"0xe3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","sha3Uncles":"0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347","size":"0x5c8","stateRoot":"0x5615e4342d231c352438f0ba6a8f0f641459f67961961764b781a909969b28ad","timestamp":"0x690be89e","transactions":[{"blockHash":"0xc463a3120c35268f610d969f5608b479332ef10953af77c7a6be806195831196","blockNumber":"0x21e3b52","depositReceiptVersion":"0x1","from":"0xdeaddeaddeaddeaddeaddeaddeaddeaddead0001","gas":"0xf4240","gasPrice":"0x0","hash":"0x613834aaeeb676fb45e312531c0cac40485a2c1b7b8ad8d648e5034dddfb5f03","input":"0x098999be000007d0000dbba0000000000000000800000000690be848000000000092042e000000000000000000000000000000000000000000000000000000000000000900000000000000000000000000000000000000000000000000000000000000010ffd7e2fb2c36e5f27c015872ce733a7b4f3fc0f4ee668d7469c557c48f8250f0000000000000000000000004ab3387810ef500bfe05a49dc53a44c222cbab3e000000000000000000000000","mint":"0x0","nonce":"0x21e3b52","r":"0x0","s":"0x0","sourceHash":"0x35dd2ec802504a143048c7830f8f570e0d6cf5147217af869939c6b4ba710a36","to":"0x4200000000000000000000000000000000000015","transactionIndex":"0x0","type":"0x7e","v":"0x0","value":"0x0"},{"accessList":[],"blockHash":"0xc463a3120c35268f610d969f5608b479332ef10953af77c7a6be806195831196","blockNumber":"0x21e3b52","chainId":"0x515","from":"0xc0043c50ba044a5f948d32faed806913d781d428","gas":"0x5578","gasPrice":"0x33","hash":"0x89f7852587dca7dd2f31b02f3a93f47669a82784025d607ec7c336b66af84a19","input":"0x426c6f636b204e756d6265723a203335353335363938","maxFeePerGas":"0x33","maxPriorityFeePerGas":"0x0","nonce":"0x1c8ea91","r":"0x91f83058c881d9ad71c179ce680326501702eb68150d20b2bf7786e388f954a2","s":"0x180185d83e503f11bf3c265c1f9296ed8d3d7c04031cd8bb30509ad188ce7bbc","to":"0x0000000000000000000000000000000000000000","transactionIndex":"0x1","type":"0x2","v":"0x0","value":"0x0","yParity":"0x0"},{"accessList":[],"blockHash":"0xc463a3120c35268f610d969f5608b479332ef10953af77c7a6be806195831196","blockNumber":"0x21e3b52","chainId":"0x515","from":"0xc0043c50ba044a5f948d32faed806913d781d428","gas":"0x8809","gasPrice":"0x33","hash":"0x8416aa3bb024ea5dce8fc00d21a500df4e782743bdf245589b5aba920324df11","input":"0x3fd553e8","maxFeePerGas":"0x33","maxPriorityFeePerGas":"0x0","nonce":"0x1c8ea92","r":"0xd73ce313aafea312e0b7244767e45f8b05d50305e0f4e4c3c564ddc751666815","s":"0x2ee015ce2363311823c0b2e96bfb0e8090fd53c6cdd99be8cf343af123036dfc","to":"0xf878f0340bf132c28f3211e8b46c569edf817495","transactionIndex":"0x2","type":"0x2","v":"0x1","value":"0x0","yParity":"0x1"},{"accessList":[],"blockHash":"0xc463a3120c35268f610d969f5608b479332ef10953af77c7a6be806195831196","blockNumber":"0x21e3b52","chainId":"0x515","from":"0xc0043c50ba044a5f948d32faed806913d781d428","gas":"0x8809","gasPrice":"0x33","hash":"0x9afeb04e8858ec2c17d1f6543fc3c00772ef05a7177733770c186cee02c67112","input":"0x3fd553e8","maxFeePerGas":"0x33","maxPriorityFeePerGas":"0x0","nonce":"0x1c8ea93","r":"0xf8cd94080642e116bc772f36a02d002505227aa542e1c13e5129ab40b8b037fb","s":"0x608318d3895388e39b218bcb275380cebc566e68f26d3d434e32b8b58366cdf","to":"0xf878f0340bf132c28f3211e8b46c569edf817495","transactionIndex":"0x3","type":"0x2","v":"0x1","value":"0x0","yParity":"0x1"},{"accessList":[],"blockHash":"0xc463a3120c35268f610d969f5608b479332ef10953af77c7a6be806195831196","blockNumber":"0x21e3b52","chainId":"0x515","from":"0xc0043c50ba044a5f948d32faed806913d781d428","gas":"0x8809","gasPrice":"0x33","hash":"0x68e4cb3cafd045680f66411af5c22bd4f239972551c668fde34fb495a07c06e7","input":"0x3fd553e8","maxFeePerGas":"0x33","maxPriorityFeePerGas":"0x0","nonce":"0x1c8ea94","r":"0x4368492ec1d087703aaf6f5fefe4427b3bf382e5cd07133f638bb6701f15fe61","s":"0x5e28757fbdc7e744118be36d5a1548eb7c009eefcb5dc5c5040e09c2fc6de9d8","to":"0xf878f0340bf132c28f3211e8b46c569edf817495","transactionIndex":"0x4","type":"0x2","v":"0x0","value":"0x0","yParity":"0x0"},{"accessList":[],"blockHash":"0xc463a3120c35268f610d969f5608b479332ef10953af77c7a6be806195831196","blockNumber":"0x21e3b52","chainId":"0x515","from":"0xc0043c50ba044a5f948d32faed806913d781d428","gas":"0x8809","gasPrice":"0x33","hash":"0x6d65b11050eee84c3f492106ca95057825d34274b5ca9bcf533885a7ff238d39","input":"0x3fd553e8","maxFeePerGas":"0x33","maxPriorityFeePerGas":"0x0","nonce":"0x1c8ea95","r":"0x802ba6d4f37e3b8de96095bd0b216144f276171d16dc62a004f1a89009af5dee","s":"0xf0c6250cfd1a062a1bc2bc353a5c227a980cac0f233b7be8932f2192342ec4f","to":"0xf878f0340bf132c28f3211e8b46c569edf817495","transactionIndex":"0x5","type":"0x2","v":"0x0","value":"0x0","yParity":"0x0"}],"transactionsRoot":"0x3dc5956bbd482687508f25d0e057282192067bb0eb80b2ac869320ef7f45cdeb","uncles":[],"withdrawals":[],"withdrawalsRoot":"0x62ed62e0391b081bf172f287fbbe75e87d8a6c22f1d3b1f1aef4788c134633d2"}
"#;

        let sequence: FlashBlockCompleteSequence = serde_json::from_str(raw_sequence).unwrap();
        let payload = OpExecutionData::from(&sequence);
    }
}
