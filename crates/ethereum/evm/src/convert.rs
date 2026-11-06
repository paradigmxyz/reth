//! Conversion helpers for Ethereum execution.

use alloy_consensus::{transaction::Recovered, BlockHeader};
use alloy_eips::eip7840::BlobParams;
use alloy_primitives::{Address, BlockNumber, BlockTimestamp, U256};
use alloy_rpc_types_engine::ExecutionData;
use evm2::{
    env::{BlockEnv, BlockEnvExt},
    ethereum::RecoveredTxEnvelope,
    SpecId,
};
use reth_chainspec::EthereumHardforks;
use reth_ethereum_primitives::TransactionSigned;
use reth_evm::{ExecutableTxParts, RecoveredTx};

#[cfg(feature = "std")]
const PARALLEL_AUTHORIZATION_THRESHOLD: usize = 16;

/// Map the latest active Ethereum hardfork at `timestamp` or `block_number` to a [`SpecId`].
pub fn spec_id_by_timestamp_and_block_number<C>(
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
    BlockEnvExt {
        number: U256::from(header.number()),
        beneficiary: header.beneficiary(),
        timestamp: U256::from(header.timestamp()),
        gas_limit: U256::from(header.gas_limit()),
        basefee: U256::from(header.base_fee_per_gas().unwrap_or_default()),
        difficulty: header.difficulty(),
        prevrandao: header.mix_hash().map(|hash| U256::from_be_bytes(hash.0)).unwrap_or_default(),
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
    spec: SpecId,
    blob_params: Option<BlobParams>,
) -> BlockEnv {
    let payload = &payload.payload;
    BlockEnvExt {
        number: U256::from(payload.block_number()),
        beneficiary: payload.fee_recipient(),
        timestamp: U256::from(payload.timestamp()),
        gas_limit: U256::from(payload.gas_limit()),
        basefee: U256::from(payload.saturated_base_fee_per_gas()),
        // Preserve the payload randao as difficulty for custom pre-Merge configurations.
        difficulty: if spec >= SpecId::MERGE {
            U256::ZERO
        } else {
            U256::from_be_bytes(payload.as_v1().prev_randao.0)
        },
        prevrandao: U256::from_be_bytes(payload.as_v1().prev_randao.0),
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

/// Recovered Ethereum transaction paired with its cached transaction environment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutableRecoveredTx {
    tx_env: RecoveredTxEnvelope,
    tx: Recovered<TransactionSigned>,
}

impl ExecutableRecoveredTx {
    /// Creates a transaction wrapper and precomputes the transaction environment.
    pub fn new(tx: Recovered<TransactionSigned>) -> Self {
        // Cache the hash before cloning so neither envelope hashes on the execution thread.
        tx.inner().tx_hash();
        let tx_env = tx.clone().map(|signed| match signed {
            TransactionSigned::Eip7702(signed) => {
                let body = signed.strip_signature();
                evm2::ethereum::TxEnvelope::Eip7702(recover_authorizations(body))
            }
            signed => signed.into(),
        });
        Self { tx_env, tx }
    }
}

/// Recovers EIP-7702 authorizations, using parallel recovery for large lists.
pub fn recover_authorizations(tx: alloy_consensus::TxEip7702) -> evm2::ethereum::LazyTxEip7702 {
    #[cfg(feature = "std")]
    if tx.authorization_list.len() >= PARALLEL_AUTHORIZATION_THRESHOLD {
        use rayon::prelude::*;

        let authorities = tx
            .authorization_list
            .par_iter()
            .map(|auth| evm2::ethereum::LazyAuthorization::Signed(auth).authority())
            .collect();
        return evm2::ethereum::LazyTxEip7702::from_cached_recovered_authorizations(tx, authorities);
    }

    evm2::ethereum::LazyTxEip7702::from_recovered_authorizations(tx)
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
        (self.tx_env, self.tx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    #[cfg(feature = "std")]
    use alloy_consensus::TxEip7702;
    #[cfg(feature = "std")]
    use alloy_eips::eip7702::{Authorization, SignedAuthorization};
    #[cfg(feature = "std")]
    use alloy_primitives::Signature;
    #[cfg(feature = "std")]
    use reth_ethereum_primitives::Transaction;

    #[test]
    fn block_env_uses_header_slot_number() {
        let header = Header { slot_number: Some(42), ..Default::default() };

        let env = block_env_with_blob_params(&header, None);

        assert_eq!(env.slot_num, U256::from(42));
    }

    #[cfg(feature = "std")]
    fn test_authorization() -> SignedAuthorization {
        SignedAuthorization::new_unchecked(
            Authorization { chain_id: U256::ZERO, address: Address::ZERO, nonce: 0 },
            0,
            U256::from(1),
            U256::from(1),
        )
    }

    #[test]
    #[cfg(feature = "std")]
    fn parallel_authority_recovery_matches_serial() {
        let mut authorizations = vec![test_authorization(); PARALLEL_AUTHORIZATION_THRESHOLD];
        authorizations[1] = SignedAuthorization::new_unchecked(
            Authorization { chain_id: U256::ZERO, address: Address::ZERO, nonce: 1 },
            0,
            U256::from(1),
            U256::from(1),
        );
        authorizations[2] = SignedAuthorization::new_unchecked(
            Authorization { chain_id: U256::ZERO, address: Address::ZERO, nonce: 2 },
            0,
            U256::ZERO,
            U256::ZERO,
        );
        let tx = TxEip7702 { authorization_list: authorizations, ..Default::default() };
        let serial: Vec<_> =
            evm2::ethereum::LazyTxEip7702::from_recovered_authorizations(tx.clone())
                .authorizations()
                .map(|auth| auth.authority())
                .collect();
        let signed =
            TransactionSigned::new_unhashed(Transaction::Eip7702(tx), Signature::test_signature());
        let recovered = Recovered::new_unchecked(signed, Address::ZERO);
        let executable = ExecutableRecoveredTx::new(recovered);
        let evm2::ethereum::TxEnvelope::Eip7702(parallel) = executable.tx_env.inner() else {
            panic!("expected EIP-7702 transaction")
        };
        assert!(parallel
            .authorizations()
            .all(|auth| matches!(auth, evm2::ethereum::LazyAuthorization::Recovered { .. })));
        assert_eq!(
            parallel.authorizations().map(|auth| auth.authority()).collect::<Vec<_>>(),
            serial
        );
    }
}
