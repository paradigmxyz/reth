use alloy_eips::eip4895::Withdrawal;
use alloy_primitives::{Address, Bloom, Bytes, B256, U256};
use alloy_rpc_types_engine::{ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3};

/// Configuration for invalidating payload fields
#[derive(Debug, Default)]
pub(super) struct InvalidationConfig {
    // Explicit value overrides (Option<T>)
    pub(super) parent_hash: Option<B256>,
    pub(super) fee_recipient: Option<Address>,
    pub(super) state_root: Option<B256>,
    pub(super) receipts_root: Option<B256>,
    pub(super) logs_bloom: Option<Bloom>,
    pub(super) prev_randao: Option<B256>,
    pub(super) block_number: Option<u64>,
    pub(super) gas_limit: Option<u64>,
    pub(super) gas_used: Option<u64>,
    pub(super) timestamp: Option<u64>,
    pub(super) extra_data: Option<Bytes>,
    pub(super) base_fee_per_gas: Option<u64>,
    pub(super) block_hash: Option<B256>,
    pub(super) blob_gas_used: Option<u64>,
    pub(super) excess_blob_gas: Option<u64>,

    // Auto-invalidation flags
    pub(super) invalidate_parent_hash: bool,
    pub(super) invalidate_state_root: bool,
    pub(super) invalidate_receipts_root: bool,
    pub(super) invalidate_gas_used: bool,
    pub(super) invalidate_block_number: bool,
    pub(super) invalidate_timestamp: bool,
    pub(super) invalidate_base_fee: bool,
    pub(super) invalidate_transactions: bool,
    pub(super) invalidate_block_hash: bool,
    pub(super) invalidate_withdrawals: bool,
    pub(super) invalidate_blob_gas_used: bool,
    pub(super) invalidate_excess_blob_gas: bool,
}

impl InvalidationConfig {
    /// Returns true if `block_hash` is being explicitly set or auto-invalidated.
    /// When true, the caller should skip recalculating the block hash since it will be overwritten.
    pub(super) const fn should_skip_hash_recalc(&self) -> bool {
        self.block_hash.is_some() || self.invalidate_block_hash
    }

    /// Applies invalidations to a V1 payload, returns list of what was changed.
    pub(super) fn apply_to_payload_v1(&self, payload: &mut ExecutionPayloadV1) -> Vec<String> {
        let mut changes = Vec::new();

        // Explicit value overrides
        if let Some(parent_hash) = self.parent_hash {
            payload.parent_hash = parent_hash;
            changes.push(format!("parent_hash = {parent_hash}"));
        }

        if let Some(fee_recipient) = self.fee_recipient {
            payload.fee_recipient = fee_recipient;
            changes.push(format!("fee_recipient = {fee_recipient}"));
        }

        if let Some(state_root) = self.state_root {
            payload.state_root = state_root;
            changes.push(format!("state_root = {state_root}"));
        }

        if let Some(receipts_root) = self.receipts_root {
            payload.receipts_root = receipts_root;
            changes.push(format!("receipts_root = {receipts_root}"));
        }

        if let Some(logs_bloom) = self.logs_bloom {
            payload.logs_bloom = logs_bloom;
            changes.push("logs_bloom = <custom>".to_string());
        }

        if let Some(prev_randao) = self.prev_randao {
            payload.prev_randao = prev_randao;
            changes.push(format!("prev_randao = {prev_randao}"));
        }

        if let Some(block_number) = self.block_number {
            payload.block_number = block_number;
            changes.push(format!("block_number = {block_number}"));
        }

        if let Some(gas_limit) = self.gas_limit {
            payload.gas_limit = gas_limit;
            changes.push(format!("gas_limit = {gas_limit}"));
        }

        if let Some(gas_used) = self.gas_used {
            payload.gas_used = gas_used;
            changes.push(format!("gas_used = {gas_used}"));
        }

        if let Some(timestamp) = self.timestamp {
            payload.timestamp = timestamp;
            changes.push(format!("timestamp = {timestamp}"));
        }

        if let Some(ref extra_data) = self.extra_data {
            payload.extra_data = extra_data.clone();
            changes.push(format!("extra_data = {} bytes", extra_data.len()));
        }

        if let Some(base_fee_per_gas) = self.base_fee_per_gas {
            payload.base_fee_per_gas = U256::from_limbs([base_fee_per_gas, 0, 0, 0]);
            changes.push(format!("base_fee_per_gas = {base_fee_per_gas}"));
        }

        if let Some(block_hash) = self.block_hash {
            payload.block_hash = block_hash;
            changes.push(format!("block_hash = {block_hash}"));
        }

        // Auto-invalidation flags
        if self.invalidate_parent_hash {
            let random_hash = B256::random();
            payload.parent_hash = random_hash;
            changes.push(format!("parent_hash = {random_hash} (auto-invalidated: random)"));
        }

        if self.invalidate_state_root {
            payload.state_root = B256::ZERO;
            changes.push("state_root = ZERO (auto-invalidated: empty trie root)".to_string());
        }

        if self.invalidate_receipts_root {
            payload.receipts_root = B256::ZERO;
            changes.push("receipts_root = ZERO (auto-invalidated)".to_string());
        }

        if self.invalidate_gas_used {
            let invalid_gas = payload.gas_limit + 1;
            payload.gas_used = invalid_gas;
            changes.push(format!("gas_used = {invalid_gas} (auto-invalidated: exceeds gas_limit)"));
        }

        if self.invalidate_block_number {
            let invalid_number = payload.block_number + 999;
            payload.block_number = invalid_number;
            changes.push(format!("block_number = {invalid_number} (auto-invalidated: huge gap)"));
        }

        if self.invalidate_timestamp {
            payload.timestamp = 0;
            changes.push("timestamp = 0 (auto-invalidated: impossibly old)".to_string());
        }

        if self.invalidate_base_fee {
            payload.base_fee_per_gas = U256::ZERO;
            changes
                .push("base_fee_per_gas = 0 (auto-invalidated: invalid post-London)".to_string());
        }

        if self.invalidate_transactions {
            let invalid_tx = Bytes::from_static(&[0xff, 0xff, 0xff]);
            payload.transactions.insert(0, invalid_tx);
            changes.push("transactions = prepended invalid RLP (auto-invalidated)".to_string());
        }

        if self.invalidate_block_hash {
            let random_hash = B256::random();
            payload.block_hash = random_hash;
            changes.push(format!("block_hash = {random_hash} (auto-invalidated: random)"));
        }

        changes
    }

    /// Applies invalidations to a V2 payload, returns list of what was changed.
    pub(super) fn apply_to_payload_v2(&self, payload: &mut ExecutionPayloadV2) -> Vec<String> {
        let mut changes = self.apply_to_payload_v1(&mut payload.payload_inner);

        // Handle withdrawals invalidation (V2+)
        if self.invalidate_withdrawals {
            let fake_withdrawal = Withdrawal {
                index: u64::MAX,
                validator_index: u64::MAX,
                address: Address::ZERO,
                amount: u64::MAX,
            };
            payload.withdrawals.push(fake_withdrawal);
            changes.push("withdrawals = added fake withdrawal (auto-invalidated)".to_string());
        }

        changes
    }

    /// Applies invalidations to a V3 payload, returns list of what was changed.
    pub(super) fn apply_to_payload_v3(&self, payload: &mut ExecutionPayloadV3) -> Vec<String> {
        let mut changes = self.apply_to_payload_v2(&mut payload.payload_inner);

        // Explicit overrides for V3 fields
        if let Some(blob_gas_used) = self.blob_gas_used {
            payload.blob_gas_used = blob_gas_used;
            changes.push(format!("blob_gas_used = {blob_gas_used}"));
        }

        if let Some(excess_blob_gas) = self.excess_blob_gas {
            payload.excess_blob_gas = excess_blob_gas;
            changes.push(format!("excess_blob_gas = {excess_blob_gas}"));
        }

        // Auto-invalidation for V3 fields
        if self.invalidate_blob_gas_used {
            payload.blob_gas_used = u64::MAX;
            changes.push("blob_gas_used = MAX (auto-invalidated)".to_string());
        }

        if self.invalidate_excess_blob_gas {
            payload.excess_blob_gas = u64::MAX;
            changes.push("excess_blob_gas = MAX (auto-invalidated)".to_string());
        }

        changes
    }
}
