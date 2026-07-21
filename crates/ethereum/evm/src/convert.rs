//! Conversion helpers for Ethereum execution.

use alloy_consensus::{
    transaction::{Recovered, TxHashRef},
    BlockHeader,
};
use alloy_eips::eip7840::BlobParams;
use alloy_primitives::{Address, BlockNumber, BlockTimestamp, B256, U256};
use alloy_rpc_types_engine::ExecutionData;
use evm2::{env::BlockEnv, ethereum::RecoveredTxEnvelope, SpecId};
use reth_chainspec::EthereumHardforks;
use reth_ethereum_primitives::TransactionSigned;
use reth_evm::{ExecutableTxParts, FromRecoveredTx, FromTxWithEncoded, RecoveredTx};

/// Map the latest active Ethereum hardfork at `timestamp` or `block_number` to a [`SpecId`].
pub(crate) fn spec_id_by_timestamp_and_block_number<C>(
    chain_spec: &C,
    timestamp: BlockTimestamp,
    block_number: BlockNumber,
) -> SpecId
where
    C: EthereumHardforks,
{
    if chain_spec.is_amsterdam_active_at_timestamp(timestamp) {
        SpecId::AMSTERDAM
    } else if chain_spec.is_osaka_active_at_timestamp(timestamp) {
        SpecId::OSAKA
    } else if chain_spec.is_prague_active_at_timestamp(timestamp) {
        SpecId::PRAGUE
    } else if chain_spec.is_cancun_active_at_timestamp(timestamp) {
        SpecId::CANCUN
    } else if chain_spec.is_shanghai_active_at_timestamp(timestamp) {
        SpecId::SHANGHAI
    } else if chain_spec.is_paris_active_at_block(block_number) {
        SpecId::MERGE
    } else if chain_spec.is_london_active_at_block(block_number) {
        SpecId::LONDON
    } else if chain_spec.is_berlin_active_at_block(block_number) {
        SpecId::BERLIN
    } else if chain_spec.is_istanbul_active_at_block(block_number) {
        SpecId::ISTANBUL
    } else if chain_spec.is_petersburg_active_at_block(block_number) {
        SpecId::PETERSBURG
    } else if chain_spec.is_byzantium_active_at_block(block_number) {
        SpecId::BYZANTIUM
    } else if chain_spec.is_spurious_dragon_active_at_block(block_number) {
        SpecId::SPURIOUS_DRAGON
    } else if chain_spec.is_tangerine_whistle_active_at_block(block_number) {
        SpecId::TANGERINE
    } else if chain_spec.is_homestead_active_at_block(block_number) {
        SpecId::HOMESTEAD
    } else {
        SpecId::FRONTIER
    }
}

/// Map the latest active hardfork at `header` to an [`SpecId`].
pub(crate) fn spec_id<C, H>(chain_spec: &C, header: &H) -> SpecId
where
    C: EthereumHardforks,
    H: BlockHeader,
{
    spec_id_by_timestamp_and_block_number(chain_spec, header.timestamp(), header.number())
}

/// Converts an Ethereum header into the block environment with chain blob parameters.
pub(crate) fn block_env_with_blob_params<H: BlockHeader>(
    header: &H,
    blob_params: Option<BlobParams>,
) -> BlockEnv {
    BlockEnv {
        number: U256::from(header.number()),
        beneficiary: header.beneficiary(),
        timestamp: U256::from(header.timestamp()),
        gas_limit: U256::from(header.gas_limit()),
        basefee: U256::from(header.base_fee_per_gas().unwrap_or_default()),
        difficulty: header.difficulty(),
        prevrandao: header
            .mix_hash()
            .map(|hash| U256::from_be_slice(hash.as_slice()))
            .unwrap_or_default(),
        blob_basefee: blob_basefee(header.excess_blob_gas(), blob_params),
        slot_num: U256::from(header.slot_number().unwrap_or_default()),
        ext: (),
        _non_exhaustive: (),
    }
}

/// Converts engine execution payload data into the block environment.
#[cfg_attr(not(feature = "std"), allow(dead_code))]
pub(crate) fn payload_block_env(
    payload: &ExecutionData,
    blob_params: Option<BlobParams>,
) -> BlockEnv {
    let payload = &payload.payload;
    BlockEnv {
        number: U256::from(payload.block_number()),
        beneficiary: payload.fee_recipient(),
        timestamp: U256::from(payload.timestamp()),
        gas_limit: U256::from(payload.gas_limit()),
        basefee: U256::from(payload.saturated_base_fee_per_gas()),
        difficulty: U256::ZERO,
        prevrandao: U256::from_be_slice(payload.as_v1().prev_randao.as_slice()),
        blob_basefee: blob_basefee(payload.excess_blob_gas(), blob_params),
        slot_num: U256::from(payload.as_v4().map(|v4| v4.slot_number).unwrap_or_default()),
        ext: (),
        _non_exhaustive: (),
    }
}

fn blob_basefee(excess_blob_gas: Option<u64>, blob_params: Option<BlobParams>) -> U256 {
    excess_blob_gas
        .zip(blob_params)
        .map(|(excess_blob_gas, params)| U256::from(params.calc_blob_fee(excess_blob_gas)))
        .unwrap_or_default()
}

/// Cached transaction environment used by engine execution and prewarming.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EthTxEnv {
    envelope: RecoveredTxEnvelope,
    tx_hash: B256,
}

impl EthTxEnv {
    /// Returns the original transaction hash.
    pub const fn tx_hash(&self) -> B256 {
        self.tx_hash
    }

    /// Consumes the wrapper and returns the transaction envelope.
    pub fn into_envelope(self) -> RecoveredTxEnvelope {
        self.envelope
    }
}

impl From<Recovered<TransactionSigned>> for EthTxEnv {
    fn from(value: Recovered<TransactionSigned>) -> Self {
        let tx_hash = *value.tx_hash();
        Self { envelope: recovered_tx_envelope(value), tx_hash }
    }
}

impl FromRecoveredTx<TransactionSigned> for EthTxEnv {
    fn from_recovered_tx(tx: Recovered<TransactionSigned>) -> Self {
        tx.into()
    }
}

impl FromTxWithEncoded<TransactionSigned> for EthTxEnv {}

/// Recovered Ethereum transaction paired with its cached transaction environment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutableRecoveredTx {
    tx_env: EthTxEnv,
    tx: Recovered<TransactionSigned>,
}

impl ExecutableRecoveredTx {
    /// Creates a transaction wrapper and precomputes the transaction environment.
    pub fn new(tx: Recovered<TransactionSigned>) -> Self {
        let tx_env = tx.clone().into();
        Self { tx_env, tx }
    }
}

impl RecoveredTx<TransactionSigned> for ExecutableRecoveredTx {
    fn tx(&self) -> &TransactionSigned {
        self.tx.inner()
    }

    fn signer(&self) -> &Address {
        self.tx.signer_ref()
    }
}

impl ExecutableTxParts<RecoveredTxEnvelope, TransactionSigned> for ExecutableRecoveredTx {
    type Recovered = Recovered<TransactionSigned>;

    fn into_parts(self) -> (RecoveredTxEnvelope, Self::Recovered) {
        (self.tx_env.into_envelope(), self.tx)
    }
}

/// Converts an owned recovered Reth Ethereum transaction into a recovered envelope.
pub(crate) fn recovered_tx_envelope(tx: Recovered<TransactionSigned>) -> RecoveredTxEnvelope {
    tx.convert()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;

    #[test]
    fn block_env_uses_header_slot_number() {
        let header = Header { slot_number: Some(42), ..Default::default() };

        let env = block_env_with_blob_params(&header, None);

        assert_eq!(env.slot_num, U256::from(42));
    }
}
