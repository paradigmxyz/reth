use abi::{STAKE_HUB_ABI, VALIDATOR_SET_ABI};
use alloy_consensus::TxLegacy;
use alloy_dyn_abi::JsonAbiExt;
use alloy_json_abi::JsonAbi;
use alloy_primitives::{BlockNumber, Bytes, TxKind, B256, U256};
use reth_chainspec::ChainSpec;
use reth_primitives::Transaction;
use reth_provider::{BlockNumReader, ProviderError};
use std::{cmp::Ordering, sync::Arc};

use crate::system_contracts::{
    CROSS_CHAIN_CONTRACT, GOVERNOR_CONTRACT, GOV_TOKEN_CONTRACT, LIGHT_CLIENT_CONTRACT,
    RELAYER_HUB_CONTRACT, RELAYER_INCENTIVIZE_CONTRACT, SLASH_CONTRACT, STAKE_HUB_CONTRACT,
    TIMELOCK_CONTRACT, TOKEN_HUB_CONTRACT, TOKEN_RECOVER_PORTAL_CONTRACT, VALIDATOR_CONTRACT,
};

mod abi;

/// Errors that can occur in Parlia consensus
#[derive(Debug, thiserror::Error)]
pub enum ParliaConsensusErr {
    /// Error from the provider
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// Head block hash not found
    #[error("Head block hash not found")]
    HeadHashNotFound,
}

/// Parlia consensus implementation
pub struct ParliaConsensus<P> {
    /// The provider for reading block information
    provider: P,
    /// The validator contract abi
    validator_abi: JsonAbi,
    /// The stake hub abi
    stake_hub_abi: JsonAbi,
    /// The chain spec
    chain_spec: Arc<ChainSpec>,
}

impl<P> ParliaConsensus<P> {
    /// Create a new Parlia consensus instance
    pub fn new(provider: P, chain_spec: Arc<ChainSpec>) -> Self {
        let validator_abi = serde_json::from_str(*VALIDATOR_SET_ABI).unwrap();
        let stake_hub_abi = serde_json::from_str(*STAKE_HUB_ABI).unwrap();
        Self { provider, validator_abi, stake_hub_abi, chain_spec }
    }
}

impl<P> ParliaConsensus<P>
where
    P: BlockNumReader + Clone,
{
    pub fn init_genesis_contracts(&self) -> Vec<Transaction> {
        let function = self.validator_abi.function("init").unwrap().first().unwrap();
        let input = function.abi_encode_input(&[]).unwrap();

        let contracts = vec![
            VALIDATOR_CONTRACT,
            SLASH_CONTRACT,
            LIGHT_CLIENT_CONTRACT,
            RELAYER_HUB_CONTRACT,
            TOKEN_HUB_CONTRACT,
            RELAYER_INCENTIVIZE_CONTRACT,
            CROSS_CHAIN_CONTRACT,
        ];

        contracts
            .into_iter()
            .map(|contract| {
                Transaction::Legacy(TxLegacy {
                    chain_id: Some(self.chain_spec.chain().id()),
                    nonce: 0,
                    gas_limit: u64::MAX / 2,
                    gas_price: 0,
                    value: U256::ZERO,
                    input: Bytes::from(input.clone()),
                    to: TxKind::Call(contract.parse().unwrap()),
                })
            })
            .collect()
    }

    pub fn init_feynman_contracts(&self) -> Vec<Transaction> {
        let function = self.stake_hub_abi.function("initialize").unwrap().first().unwrap();
        let input = function.abi_encode_input(&[]).unwrap();

        let contracts = vec![
            STAKE_HUB_CONTRACT,
            GOVERNOR_CONTRACT,
            GOV_TOKEN_CONTRACT,
            TIMELOCK_CONTRACT,
            TOKEN_RECOVER_PORTAL_CONTRACT,
        ];

        contracts
            .into_iter()
            .map(|contract| {
                Transaction::Legacy(TxLegacy {
                    chain_id: Some(self.chain_spec.chain().id()),
                    nonce: 0,
                    gas_limit: u64::MAX / 2,
                    gas_price: 0,
                    value: U256::ZERO,
                    input: Bytes::from(input.clone()),
                    to: TxKind::Call(contract.parse().unwrap()),
                })
            })
            .collect()
    }

    /// Determines the head block hash according to Parlia consensus rules:
    /// 1. Follow the highest block number
    /// 2. For same height blocks, pick the one with lower hash
    pub(crate) fn canonical_head(
        &self,
        hash: B256,
        number: BlockNumber,
    ) -> Result<B256, ParliaConsensusErr> {
        let current_head = self.provider.best_block_number()?;
        let current_hash =
            self.provider.block_hash(current_head)?.ok_or(ParliaConsensusErr::HeadHashNotFound)?;

        match number.cmp(&current_head) {
            Ordering::Greater => Ok(hash),
            Ordering::Equal => Ok(hash.min(current_hash)),
            Ordering::Less => Ok(current_hash),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use alloy_primitives::hex;
    use reth_chainspec::ChainInfo;
    use reth_provider::BlockHashReader;

    use crate::chainspec::bsc::bsc_mainnet;

    use super::*;

    #[derive(Clone)]
    struct MockProvider {
        blocks: HashMap<BlockNumber, B256>,
        head_number: BlockNumber,
        head_hash: B256,
    }

    impl MockProvider {
        fn new(head_number: BlockNumber, head_hash: B256) -> Self {
            let mut blocks = HashMap::new();
            blocks.insert(head_number, head_hash);
            Self { blocks, head_number, head_hash }
        }
    }

    impl BlockHashReader for MockProvider {
        fn block_hash(&self, number: BlockNumber) -> Result<Option<B256>, ProviderError> {
            Ok(self.blocks.get(&number).copied())
        }

        fn canonical_hashes_range(
            &self,
            _start: BlockNumber,
            _end: BlockNumber,
        ) -> Result<Vec<B256>, ProviderError> {
            Ok(vec![])
        }
    }

    impl BlockNumReader for MockProvider {
        fn chain_info(&self) -> Result<ChainInfo, ProviderError> {
            Ok(ChainInfo { best_hash: self.head_hash, best_number: self.head_number })
        }

        fn best_block_number(&self) -> Result<BlockNumber, ProviderError> {
            Ok(self.head_number)
        }

        fn last_block_number(&self) -> Result<BlockNumber, ProviderError> {
            Ok(self.head_number)
        }

        fn block_number(&self, hash: B256) -> Result<Option<BlockNumber>, ProviderError> {
            Ok(self.blocks.iter().find_map(|(num, h)| (*h == hash).then_some(*num)))
        }
    }

    #[test]
    fn test_canonical_head() {
        let hash1 = B256::from_slice(&hex!(
            "1111111111111111111111111111111111111111111111111111111111111111"
        ));
        let hash2 = B256::from_slice(&hex!(
            "2222222222222222222222222222222222222222222222222222222222222222"
        ));

        let test_cases = [
            ((hash1, 2, 1, hash2), hash1), // Higher block wins
            ((hash1, 1, 2, hash2), hash2), // Lower block stays
            ((hash1, 1, 1, hash2), hash1), // Same height, lower hash wins
            ((hash2, 1, 1, hash1), hash1), // Same height, lower hash stays
        ];

        for ((curr_hash, curr_num, head_num, head_hash), expected) in test_cases {
            let provider = MockProvider::new(head_num, head_hash);
            let consensus = ParliaConsensus::new(provider, bsc_mainnet());
            assert_eq!(consensus.canonical_head(curr_hash, curr_num).unwrap(), expected);
        }
    }
}
