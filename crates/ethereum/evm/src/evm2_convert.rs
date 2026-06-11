//! Conversion helpers for feeding Reth Ethereum primitives into evm2.

use alloy_consensus::{transaction::Recovered, BlockHeader};
use alloy_eips::eip7840::BlobParams;
use alloy_primitives::{Address, BlockNumber, BlockTimestamp, U256};
use alloy_rpc_types_engine::ExecutionData;
use evm2::{
    env::BlockEnv,
    ethereum::{LazyTxEip7702, RecoveredTxEnvelope},
    SpecId,
};
use reth_chainspec::EthereumHardforks;
use reth_ethereum_primitives::TransactionSigned;
use reth_evm::execute::{ExecutableTxParts, RecoveredTx};

/// Map the latest active Ethereum hardfork at `timestamp` or `block_number` to an evm2 [`SpecId`].
pub fn evm2_spec_by_timestamp_and_block_number<C>(
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

/// Map the latest active hardfork at `header` to an evm2 [`SpecId`].
pub fn evm2_spec<C, H>(chain_spec: &C, header: &H) -> SpecId
where
    C: EthereumHardforks,
    H: BlockHeader,
{
    evm2_spec_by_timestamp_and_block_number(chain_spec, header.timestamp(), header.number())
}

/// Converts an Ethereum header into evm2's block environment.
pub fn evm2_block_env<H: BlockHeader>(header: &H) -> BlockEnv {
    evm2_block_env_with_blob_params(header, None)
}

/// Converts an Ethereum header into evm2's block environment with chain blob parameters.
pub fn evm2_block_env_with_blob_params<H: BlockHeader>(
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
        slot_num: U256::ZERO,
        ext: (),
        _non_exhaustive: (),
    }
}

/// Converts engine execution payload data into evm2's block environment.
pub fn evm2_payload_block_env(
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

/// Cached evm2 transaction environment used by engine execution and prewarming.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Evm2TxEnv(RecoveredTxEnvelope);

impl Evm2TxEnv {
    /// Returns the wrapped evm2 transaction envelope.
    pub const fn as_envelope(&self) -> &RecoveredTxEnvelope {
        &self.0
    }

    /// Consumes the wrapper and returns the evm2 transaction envelope.
    pub fn into_envelope(self) -> RecoveredTxEnvelope {
        self.0
    }
}

impl From<Recovered<TransactionSigned>> for Evm2TxEnv {
    fn from(value: Recovered<TransactionSigned>) -> Self {
        Self(evm2_recovered_tx(value))
    }
}

/// Recovered Ethereum transaction paired with its cached evm2 transaction environment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Evm2RecoveredTx {
    tx_env: Evm2TxEnv,
    tx: Recovered<TransactionSigned>,
}

impl Evm2RecoveredTx {
    /// Creates a transaction wrapper and precomputes the evm2 transaction environment.
    pub fn new(tx: Recovered<TransactionSigned>) -> Self {
        let tx_env = tx.clone().into();
        Self { tx_env, tx }
    }
}

impl RecoveredTx<TransactionSigned> for Evm2RecoveredTx {
    fn tx(&self) -> &TransactionSigned {
        self.tx.inner()
    }

    fn signer(&self) -> &Address {
        self.tx.signer_ref()
    }
}

impl ExecutableTxParts<Evm2TxEnv, TransactionSigned> for Evm2RecoveredTx {
    type Recovered = Recovered<TransactionSigned>;

    fn into_parts(self) -> (Evm2TxEnv, Self::Recovered) {
        (self.tx_env, self.tx)
    }
}

/// Converts an owned recovered Reth Ethereum transaction into evm2's recovered envelope.
pub fn evm2_recovered_tx(tx: Recovered<TransactionSigned>) -> RecoveredTxEnvelope {
    let (tx, signer) = tx.into_parts();
    match tx {
        TransactionSigned::Legacy(tx) => {
            RecoveredTxEnvelope::Legacy(Recovered::new_unchecked(tx.strip_signature(), signer))
        }
        TransactionSigned::Eip2930(tx) => {
            RecoveredTxEnvelope::Eip2930(Recovered::new_unchecked(tx.strip_signature(), signer))
        }
        TransactionSigned::Eip1559(tx) => {
            RecoveredTxEnvelope::Eip1559(Recovered::new_unchecked(tx.strip_signature(), signer))
        }
        TransactionSigned::Eip4844(tx) => RecoveredTxEnvelope::Eip4844(Recovered::new_unchecked(
            tx.strip_signature().into(),
            signer,
        )),
        TransactionSigned::Eip7702(tx) => RecoveredTxEnvelope::Eip7702(Recovered::new_unchecked(
            LazyTxEip7702::from_recovered_authorizations(tx.strip_signature()),
            signer,
        )),
    }
}

/// Converts a borrowed recovered Reth Ethereum transaction into evm2's recovered envelope.
pub fn evm2_recovered_tx_ref(tx: Recovered<&TransactionSigned>) -> RecoveredTxEnvelope {
    evm2_recovered_tx(Recovered::new_unchecked((*tx.inner()).clone(), tx.signer()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_eips::eip7840::BlobParams;

    #[test]
    fn evm2_block_env_uses_blob_params_for_blob_basefee() {
        let blob_params = BlobParams::cancun();
        let excess_blob_gas = 1_000_000;
        let header = Header { excess_blob_gas: Some(excess_blob_gas), ..Default::default() };

        let env = evm2_block_env_with_blob_params(&header, Some(blob_params));

        assert_eq!(env.blob_basefee, U256::from(blob_params.calc_blob_fee(excess_blob_gas)));
    }

    #[test]
    fn evm2_block_env_defaults_blob_basefee_without_blob_context() {
        let header = Header { excess_blob_gas: Some(1_000_000), ..Default::default() };

        let env = evm2_block_env(&header);

        assert_eq!(env.blob_basefee, U256::ZERO);
    }
}
