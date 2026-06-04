//! Conversion helpers for feeding Reth Ethereum primitives into evm2.

use alloy_consensus::{transaction::Recovered, BlockHeader};
use alloy_primitives::{BlockNumber, BlockTimestamp, U256};
use evm2::{env::BlockEnv, ethereum::RecoveredTxEnvelope, SpecId};
use reth_chainspec::EthereumHardforks;
use reth_ethereum_primitives::TransactionSigned;

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
        blob_basefee: U256::ZERO,
        slot_num: U256::ZERO,
        ext: (),
        _non_exhaustive: (),
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
        TransactionSigned::Eip7702(tx) => {
            RecoveredTxEnvelope::Eip7702(Recovered::new_unchecked(tx.strip_signature(), signer))
        }
    }
}

/// Converts a borrowed recovered Reth Ethereum transaction into evm2's recovered envelope.
pub fn evm2_recovered_tx_ref(tx: Recovered<&TransactionSigned>) -> RecoveredTxEnvelope {
    evm2_recovered_tx(Recovered::new_unchecked((*tx.inner()).clone(), tx.signer()))
}
