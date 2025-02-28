//! EVM config for vanilla optimism.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::sync::Arc;
use alloy_consensus::BlockHeader;
use alloy_evm::FromRecoveredTx;
use alloy_op_evm::OpEvmFactory;
use alloy_primitives::U256;
use core::fmt::Debug;
use op_alloy_consensus::EIP1559ParamError;
use reth_chainspec::EthChainSpec;
use reth_evm::{ConfigureEvm, ConfigureEvmEnv, EvmEnv, NextBlockEnvAttributes};
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_consensus::next_block_base_fee;
use reth_optimism_forks::OpHardforks;
use reth_optimism_primitives::OpPrimitives;
use reth_primitives_traits::NodePrimitives;
use revm::{
    context::{BlockEnv, CfgEnv, TxEnv},
    context_interface::block::BlobExcessGasAndPrice,
    specification::hardfork::SpecId,
};
use revm_optimism::{OpHaltReason, OpSpecId, OpTransaction};

mod config;
pub use config::{revm_spec, revm_spec_by_timestamp_after_bedrock};
mod execute;
pub use execute::*;
pub mod l1;
pub use l1::*;
mod receipts;
pub use receipts::*;

mod error;
pub use error::OpBlockExecutionError;

/// Optimism-related EVM configuration.
#[derive(Debug)]
pub struct OpEvmConfig<ChainSpec = OpChainSpec, N: NodePrimitives = OpPrimitives> {
    chain_spec: Arc<ChainSpec>,
    evm_factory: OpEvmFactory,
    receipt_builder: Arc<dyn OpReceiptBuilder<N::SignedTx, OpHaltReason, Receipt = N::Receipt>>,
}

impl<ChainSpec, N: NodePrimitives> Clone for OpEvmConfig<ChainSpec, N> {
    fn clone(&self) -> Self {
        Self {
            chain_spec: self.chain_spec.clone(),
            evm_factory: OpEvmFactory::default(),
            receipt_builder: self.receipt_builder.clone(),
        }
    }
}

impl<ChainSpec> OpEvmConfig<ChainSpec> {
    /// Creates a new [`OpEvmConfig`] with the given chain spec for OP chains.
    pub fn optimism(chain_spec: Arc<ChainSpec>) -> Self {
        Self::new(chain_spec, BasicOpReceiptBuilder::default())
    }
}

impl<ChainSpec, N: NodePrimitives> OpEvmConfig<ChainSpec, N> {
    /// Creates a new [`OpEvmConfig`] with the given chain spec.
    pub fn new(
        chain_spec: Arc<ChainSpec>,
        receipt_builder: impl OpReceiptBuilder<N::SignedTx, OpHaltReason, Receipt = N::Receipt>,
    ) -> Self {
        Self {
            chain_spec,
            evm_factory: OpEvmFactory::default(),
            receipt_builder: Arc::new(receipt_builder),
        }
    }

    /// Returns the chain spec associated with this configuration.
    pub const fn chain_spec(&self) -> &Arc<ChainSpec> {
        &self.chain_spec
    }
}

impl<ChainSpec, N> ConfigureEvmEnv for OpEvmConfig<ChainSpec, N>
where
    ChainSpec: EthChainSpec + OpHardforks,
    N: NodePrimitives,
    OpTransaction<TxEnv>: FromRecoveredTx<N::SignedTx>,
{
    type Header = N::BlockHeader;
    type Transaction = N::SignedTx;
    type Error = EIP1559ParamError;
    type TxEnv = OpTransaction<TxEnv>;
    type Spec = OpSpecId;

    fn evm_env(&self, header: &Self::Header) -> EvmEnv<Self::Spec> {
        let spec = config::revm_spec(self.chain_spec(), header);

        let cfg_env = CfgEnv::new().with_chain_id(self.chain_spec.chain().id()).with_spec(spec);

        let block_env = BlockEnv {
            number: header.number(),
            beneficiary: header.beneficiary(),
            timestamp: header.timestamp(),
            difficulty: if spec.into_eth_spec() >= SpecId::MERGE {
                U256::ZERO
            } else {
                header.difficulty()
            },
            prevrandao: if spec.into_eth_spec() >= SpecId::MERGE {
                header.mix_hash()
            } else {
                None
            },
            gas_limit: header.gas_limit(),
            basefee: header.base_fee_per_gas().unwrap_or_default(),
            // EIP-4844 excess blob gas of this block, introduced in Cancun
            blob_excess_gas_and_price: header.excess_blob_gas().map(|excess_blob_gas| {
                BlobExcessGasAndPrice::new(excess_blob_gas, spec.into_eth_spec() >= SpecId::PRAGUE)
            }),
        };

        EvmEnv { cfg_env, block_env }
    }

    fn next_evm_env(
        &self,
        parent: &Self::Header,
        attributes: NextBlockEnvAttributes<'_>,
    ) -> Result<EvmEnv<Self::Spec>, Self::Error> {
        // ensure we're not missing any timestamp based hardforks
        let spec_id = revm_spec_by_timestamp_after_bedrock(&self.chain_spec, attributes.timestamp);

        // configure evm env based on parent block
        let cfg_env = CfgEnv::new().with_chain_id(self.chain_spec.chain().id()).with_spec(spec_id);

        // if the parent block did not have excess blob gas (i.e. it was pre-cancun), but it is
        // cancun now, we need to set the excess blob gas to the default value(0)
        let blob_excess_gas_and_price = parent
            .maybe_next_block_excess_blob_gas(
                self.chain_spec().blob_params_at_timestamp(attributes.timestamp),
            )
            .or_else(|| (spec_id.into_eth_spec().is_enabled_in(SpecId::CANCUN)).then_some(0))
            .map(|gas| BlobExcessGasAndPrice::new(gas, false));

        let block_env = BlockEnv {
            number: parent.number() + 1,
            beneficiary: attributes.suggested_fee_recipient,
            timestamp: attributes.timestamp,
            difficulty: U256::ZERO,
            prevrandao: Some(attributes.prev_randao),
            gas_limit: attributes.gas_limit,
            // calculate basefee based on parent block's gas usage
            basefee: next_block_base_fee(&self.chain_spec, parent, attributes.timestamp)?,
            // calculate excess gas based on parent block's blob gas usage
            blob_excess_gas_and_price,
        };

        Ok(EvmEnv { cfg_env, block_env })
    }
}

impl<ChainSpec, N> ConfigureEvm for OpEvmConfig<ChainSpec, N>
where
    ChainSpec: EthChainSpec + OpHardforks,
    N: NodePrimitives,
    OpTransaction<TxEnv>: FromRecoveredTx<N::SignedTx>,
{
    type EvmFactory = OpEvmFactory;

    fn evm_factory(&self) -> &Self::EvmFactory {
        &self.evm_factory
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Header, Receipt};
    use alloy_eips::eip7685::Requests;
    use alloy_genesis::Genesis;
    use alloy_primitives::{bytes, map::HashMap, Address, LogData, B256};
    use reth_chainspec::ChainSpec;
    use reth_evm::execute::ProviderError;
    use reth_execution_types::{
        AccountRevertInit, BundleStateInit, Chain, ExecutionOutcome, RevertsInit,
    };
    use reth_optimism_chainspec::BASE_MAINNET;
    use reth_optimism_primitives::{OpBlock, OpPrimitives, OpReceipt};
    use reth_primitives_traits::{Account, RecoveredBlock};
    use revm::{database_interface::EmptyDBTyped, inspector::NoOpInspector, state::AccountInfo};
    use revm_database::{BundleState, CacheDB};
    use revm_optimism::OpSpecId;
    use revm_primitives::Log;
    use std::sync::Arc;

    fn test_evm_config() -> OpEvmConfig {
        OpEvmConfig::optimism(BASE_MAINNET.clone())
    }

    #[test]
    fn test_fill_cfg_and_block_env() {
        // Create a default header
        let header = Header::default();

        // Build the ChainSpec for Ethereum mainnet, activating London, Paris, and Shanghai
        // hardforks
        let chain_spec = ChainSpec::builder()
            .chain(0.into())
            .genesis(Genesis::default())
            .london_activated()
            .paris_activated()
            .shanghai_activated()
            .build();

        // Use the `OpEvmConfig` to create the `cfg_env` and `block_env` based on the ChainSpec,
        // Header, and total difficulty
        let EvmEnv { cfg_env, .. } =
            OpEvmConfig::optimism(Arc::new(OpChainSpec { inner: chain_spec.clone() }))
                .evm_env(&header);

        // Assert that the chain ID in the `cfg_env` is correctly set to the chain ID of the
        // ChainSpec
        assert_eq!(cfg_env.chain_id, chain_spec.chain().id());
    }

    #[test]
    fn test_evm_with_env_default_spec() {
        let evm_config = test_evm_config();

        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        let evm_env = EvmEnv::default();

        let evm = evm_config.evm_with_env(db, evm_env.clone());

        // Check that the EVM environment
        assert_eq!(evm.cfg, evm_env.cfg_env);
    }

    #[test]
    fn test_evm_with_env_custom_cfg() {
        let evm_config = test_evm_config();

        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        // Create a custom configuration environment with a chain ID of 111
        let cfg = CfgEnv::new().with_chain_id(111).with_spec(OpSpecId::default());

        let evm_env = EvmEnv { cfg_env: cfg.clone(), ..Default::default() };

        let evm = evm_config.evm_with_env(db, evm_env);

        // Check that the EVM environment is initialized with the custom environment
        assert_eq!(evm.cfg, cfg);
    }

    #[test]
    fn test_evm_with_env_custom_block_and_tx() {
        let evm_config = test_evm_config();

        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        // Create customs block and tx env
        let block =
            BlockEnv { basefee: 1000, gas_limit: 10_000_000, number: 42, ..Default::default() };

        let evm_env = EvmEnv { block_env: block, ..Default::default() };

        let evm = evm_config.evm_with_env(db, evm_env.clone());

        // Verify that the block and transaction environments are set correctly
        assert_eq!(evm.block, evm_env.block_env);
    }

    #[test]
    fn test_evm_with_spec_id() {
        let evm_config = test_evm_config();

        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        let evm_env =
            EvmEnv { cfg_env: CfgEnv::new().with_spec(OpSpecId::ECOTONE), ..Default::default() };

        let evm = evm_config.evm_with_env(db, evm_env.clone());

        assert_eq!(evm.cfg, evm_env.cfg_env);
    }

    #[test]
    fn test_evm_with_env_and_default_inspector() {
        let evm_config = test_evm_config();
        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        let evm_env = EvmEnv { cfg_env: Default::default(), ..Default::default() };

        let evm = evm_config.evm_with_env_and_inspector(db, evm_env.clone(), NoOpInspector {});

        // Check that the EVM environment is set to default values
        assert_eq!(evm.block, evm_env.block_env);
        assert_eq!(evm.cfg, evm_env.cfg_env);
    }

    #[test]
    fn test_evm_with_env_inspector_and_custom_cfg() {
        let evm_config = test_evm_config();
        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        let cfg = CfgEnv::new().with_chain_id(111).with_spec(OpSpecId::default());
        let block = BlockEnv::default();
        let evm_env = EvmEnv { block_env: block, cfg_env: cfg.clone() };

        let evm = evm_config.evm_with_env_and_inspector(db, evm_env.clone(), NoOpInspector {});

        // Check that the EVM environment is set with custom configuration
        assert_eq!(evm.cfg, cfg);
        assert_eq!(evm.block, evm_env.block_env);
    }

    #[test]
    fn test_evm_with_env_inspector_and_custom_block_tx() {
        let evm_config = test_evm_config();
        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        // Create custom block and tx environment
        let block =
            BlockEnv { basefee: 1000, gas_limit: 10_000_000, number: 42, ..Default::default() };
        let evm_env = EvmEnv { block_env: block, ..Default::default() };

        let evm = evm_config.evm_with_env_and_inspector(db, evm_env.clone(), NoOpInspector {});

        // Verify that the block and transaction environments are set correctly
        assert_eq!(evm.block, evm_env.block_env);
    }

    #[test]
    fn test_evm_with_env_inspector_and_spec_id() {
        let evm_config = test_evm_config();
        let db = CacheDB::<EmptyDBTyped<ProviderError>>::default();

        let evm_env =
            EvmEnv { cfg_env: CfgEnv::new().with_spec(OpSpecId::ECOTONE), ..Default::default() };

        let evm = evm_config.evm_with_env_and_inspector(db, evm_env.clone(), NoOpInspector {});

        // Check that the spec ID is set properly
        assert_eq!(evm.cfg, evm_env.cfg_env);
        assert_eq!(evm.block, evm_env.block_env);
    }

    #[test]
    fn receipts_by_block_hash() {
        // Create a default recovered block
        let block: RecoveredBlock<OpBlock> = Default::default();

        // Define block hashes for block1 and block2
        let block1_hash = B256::new([0x01; 32]);
        let block2_hash = B256::new([0x02; 32]);

        // Clone the default block into block1 and block2
        let mut block1 = block.clone();
        let mut block2 = block;

        // Set the hashes of block1 and block2
        block1.set_block_number(10);
        block1.set_hash(block1_hash);

        block2.set_block_number(11);
        block2.set_hash(block2_hash);

        // Create a random receipt object, receipt1
        let receipt1 = OpReceipt::Legacy(Receipt {
            cumulative_gas_used: 46913,
            logs: vec![],
            status: true.into(),
        });

        // Create another random receipt object, receipt2
        let receipt2 = OpReceipt::Legacy(Receipt {
            cumulative_gas_used: 1325345,
            logs: vec![],
            status: true.into(),
        });

        // Create a Receipts object with a vector of receipt vectors
        let receipts = vec![vec![receipt1.clone()], vec![receipt2]];

        // Create an ExecutionOutcome object with the created bundle, receipts, an empty requests
        // vector, and first_block set to 10
        let execution_outcome = ExecutionOutcome::<OpReceipt> {
            bundle: Default::default(),
            receipts,
            requests: vec![],
            first_block: 10,
        };

        // Create a Chain object with a BTreeMap of blocks mapped to their block numbers,
        // including block1_hash and block2_hash, and the execution_outcome
        let chain: Chain<OpPrimitives> =
            Chain::new([block1, block2], execution_outcome.clone(), None);

        // Assert that the proper receipt vector is returned for block1_hash
        assert_eq!(chain.receipts_by_block_hash(block1_hash), Some(vec![&receipt1]));

        // Create an ExecutionOutcome object with a single receipt vector containing receipt1
        let execution_outcome1 = ExecutionOutcome {
            bundle: Default::default(),
            receipts: vec![vec![receipt1]],
            requests: vec![],
            first_block: 10,
        };

        // Assert that the execution outcome at the first block contains only the first receipt
        assert_eq!(chain.execution_outcome_at_block(10), Some(execution_outcome1));

        // Assert that the execution outcome at the tip block contains the whole execution outcome
        assert_eq!(chain.execution_outcome_at_block(11), Some(execution_outcome));
    }

    #[test]
    fn test_initialisation() {
        // Create a new BundleState object with initial data
        let bundle = BundleState::new(
            vec![(Address::new([2; 20]), None, Some(AccountInfo::default()), HashMap::default())],
            vec![vec![(Address::new([2; 20]), None, vec![])]],
            vec![],
        );

        // Create a Receipts object with a vector of receipt vectors
        let receipts = vec![vec![Some(OpReceipt::Legacy(Receipt {
            cumulative_gas_used: 46913,
            logs: vec![],
            status: true.into(),
        }))]];

        // Create a Requests object with a vector of requests
        let requests = vec![Requests::new(vec![bytes!("dead"), bytes!("beef"), bytes!("beebee")])];

        // Define the first block number
        let first_block = 123;

        // Create a ExecutionOutcome object with the created bundle, receipts, requests, and
        // first_block
        let exec_res = ExecutionOutcome {
            bundle: bundle.clone(),
            receipts: receipts.clone(),
            requests: requests.clone(),
            first_block,
        };

        // Assert that creating a new ExecutionOutcome using the constructor matches exec_res
        assert_eq!(
            ExecutionOutcome::new(bundle, receipts.clone(), first_block, requests.clone()),
            exec_res
        );

        // Create a BundleStateInit object and insert initial data
        let mut state_init: BundleStateInit = HashMap::default();
        state_init
            .insert(Address::new([2; 20]), (None, Some(Account::default()), HashMap::default()));

        // Create a HashMap for account reverts and insert initial data
        let mut revert_inner: HashMap<Address, AccountRevertInit> = HashMap::default();
        revert_inner.insert(Address::new([2; 20]), (None, vec![]));

        // Create a RevertsInit object and insert the revert_inner data
        let mut revert_init: RevertsInit = HashMap::default();
        revert_init.insert(123, revert_inner);

        // Assert that creating a new ExecutionOutcome using the new_init method matches
        // exec_res
        assert_eq!(
            ExecutionOutcome::new_init(
                state_init,
                revert_init,
                vec![],
                receipts,
                first_block,
                requests,
            ),
            exec_res
        );
    }

    #[test]
    fn test_block_number_to_index() {
        // Create a Receipts object with a vector of receipt vectors
        let receipts = vec![vec![Some(OpReceipt::Legacy(Receipt {
            cumulative_gas_used: 46913,
            logs: vec![],
            status: true.into(),
        }))]];

        // Define the first block number
        let first_block = 123;

        // Create a ExecutionOutcome object with the created bundle, receipts, requests, and
        // first_block
        let exec_res = ExecutionOutcome {
            bundle: Default::default(),
            receipts,
            requests: vec![],
            first_block,
        };

        // Test before the first block
        assert_eq!(exec_res.block_number_to_index(12), None);

        // Test after after the first block but index larger than receipts length
        assert_eq!(exec_res.block_number_to_index(133), None);

        // Test after the first block
        assert_eq!(exec_res.block_number_to_index(123), Some(0));
    }

    #[test]
    fn test_get_logs() {
        // Create a Receipts object with a vector of receipt vectors
        let receipts = vec![vec![OpReceipt::Legacy(Receipt {
            cumulative_gas_used: 46913,
            logs: vec![Log::<LogData>::default()],
            status: true.into(),
        })]];

        // Define the first block number
        let first_block = 123;

        // Create a ExecutionOutcome object with the created bundle, receipts, requests, and
        // first_block
        let exec_res = ExecutionOutcome {
            bundle: Default::default(),
            receipts,
            requests: vec![],
            first_block,
        };

        // Get logs for block number 123
        let logs: Vec<&Log> = exec_res.logs(123).unwrap().collect();

        // Assert that the logs match the expected logs
        assert_eq!(logs, vec![&Log::<LogData>::default()]);
    }

    #[test]
    fn test_receipts_by_block() {
        // Create a Receipts object with a vector of receipt vectors
        let receipts = vec![vec![Some(OpReceipt::Legacy(Receipt {
            cumulative_gas_used: 46913,
            logs: vec![Log::<LogData>::default()],
            status: true.into(),
        }))]];

        // Define the first block number
        let first_block = 123;

        // Create a ExecutionOutcome object with the created bundle, receipts, requests, and
        // first_block
        let exec_res = ExecutionOutcome {
            bundle: Default::default(), // Default value for bundle
            receipts,                   // Include the created receipts
            requests: vec![],           // Empty vector for requests
            first_block,                // Set the first block number
        };

        // Get receipts for block number 123 and convert the result into a vector
        let receipts_by_block: Vec<_> = exec_res.receipts_by_block(123).iter().collect();

        // Assert that the receipts for block number 123 match the expected receipts
        assert_eq!(
            receipts_by_block,
            vec![&Some(OpReceipt::Legacy(Receipt {
                cumulative_gas_used: 46913,
                logs: vec![Log::<LogData>::default()],
                status: true.into(),
            }))]
        );
    }

    #[test]
    fn test_receipts_len() {
        // Create a Receipts object with a vector of receipt vectors
        let receipts = vec![vec![Some(OpReceipt::Legacy(Receipt {
            cumulative_gas_used: 46913,
            logs: vec![Log::<LogData>::default()],
            status: true.into(),
        }))]];

        // Create an empty Receipts object
        let receipts_empty = vec![];

        // Define the first block number
        let first_block = 123;

        // Create a ExecutionOutcome object with the created bundle, receipts, requests, and
        // first_block
        let exec_res = ExecutionOutcome {
            bundle: Default::default(), // Default value for bundle
            receipts,                   // Include the created receipts
            requests: vec![],           // Empty vector for requests
            first_block,                // Set the first block number
        };

        // Assert that the length of receipts in exec_res is 1
        assert_eq!(exec_res.len(), 1);

        // Assert that exec_res is not empty
        assert!(!exec_res.is_empty());

        // Create a ExecutionOutcome object with an empty Receipts object
        let exec_res_empty_receipts: ExecutionOutcome<OpReceipt> = ExecutionOutcome {
            bundle: Default::default(), // Default value for bundle
            receipts: receipts_empty,   // Include the empty receipts
            requests: vec![],           // Empty vector for requests
            first_block,                // Set the first block number
        };

        // Assert that the length of receipts in exec_res_empty_receipts is 0
        assert_eq!(exec_res_empty_receipts.len(), 0);

        // Assert that exec_res_empty_receipts is empty
        assert!(exec_res_empty_receipts.is_empty());
    }

    #[test]
    fn test_revert_to() {
        // Create a random receipt object
        let receipt = OpReceipt::Legacy(Receipt {
            cumulative_gas_used: 46913,
            logs: vec![],
            status: true.into(),
        });

        // Create a Receipts object with a vector of receipt vectors
        let receipts = vec![vec![Some(receipt.clone())], vec![Some(receipt.clone())]];

        // Define the first block number
        let first_block = 123;

        // Create a request.
        let request = bytes!("deadbeef");

        // Create a vector of Requests containing the request.
        let requests =
            vec![Requests::new(vec![request.clone()]), Requests::new(vec![request.clone()])];

        // Create a ExecutionOutcome object with the created bundle, receipts, requests, and
        // first_block
        let mut exec_res =
            ExecutionOutcome { bundle: Default::default(), receipts, requests, first_block };

        // Assert that the revert_to method returns true when reverting to the initial block number.
        assert!(exec_res.revert_to(123));

        // Assert that the receipts are properly cut after reverting to the initial block number.
        assert_eq!(exec_res.receipts, vec![vec![Some(receipt)]]);

        // Assert that the requests are properly cut after reverting to the initial block number.
        assert_eq!(exec_res.requests, vec![Requests::new(vec![request])]);

        // Assert that the revert_to method returns false when attempting to revert to a block
        // number greater than the initial block number.
        assert!(!exec_res.revert_to(133));

        // Assert that the revert_to method returns false when attempting to revert to a block
        // number less than the initial block number.
        assert!(!exec_res.revert_to(10));
    }

    #[test]
    fn test_extend_execution_outcome() {
        // Create a Receipt object with specific attributes.
        let receipt = OpReceipt::Legacy(Receipt {
            cumulative_gas_used: 46913,
            logs: vec![],
            status: true.into(),
        });

        // Create a Receipts object containing the receipt.
        let receipts = vec![vec![Some(receipt.clone())]];

        // Create a request.
        let request = bytes!("deadbeef");

        // Create a vector of Requests containing the request.
        let requests = vec![Requests::new(vec![request.clone()])];

        // Define the initial block number.
        let first_block = 123;

        // Create an ExecutionOutcome object.
        let mut exec_res =
            ExecutionOutcome { bundle: Default::default(), receipts, requests, first_block };

        // Extend the ExecutionOutcome object by itself.
        exec_res.extend(exec_res.clone());

        // Assert the extended ExecutionOutcome matches the expected outcome.
        assert_eq!(
            exec_res,
            ExecutionOutcome {
                bundle: Default::default(),
                receipts: vec![vec![Some(receipt.clone())], vec![Some(receipt)]],
                requests: vec![Requests::new(vec![request.clone()]), Requests::new(vec![request])],
                first_block: 123,
            }
        );
    }

    #[test]
    fn test_split_at_execution_outcome() {
        // Create a random receipt object
        let receipt = OpReceipt::Legacy(Receipt {
            cumulative_gas_used: 46913,
            logs: vec![],
            status: true.into(),
        });

        // Create a Receipts object with a vector of receipt vectors
        let receipts = vec![
            vec![Some(receipt.clone())],
            vec![Some(receipt.clone())],
            vec![Some(receipt.clone())],
        ];

        // Define the first block number
        let first_block = 123;

        // Create a request.
        let request = bytes!("deadbeef");

        // Create a vector of Requests containing the request.
        let requests = vec![
            Requests::new(vec![request.clone()]),
            Requests::new(vec![request.clone()]),
            Requests::new(vec![request.clone()]),
        ];

        // Create a ExecutionOutcome object with the created bundle, receipts, requests, and
        // first_block
        let exec_res =
            ExecutionOutcome { bundle: Default::default(), receipts, requests, first_block };

        // Split the ExecutionOutcome at block number 124
        let result = exec_res.clone().split_at(124);

        // Define the expected lower ExecutionOutcome after splitting
        let lower_execution_outcome = ExecutionOutcome {
            bundle: Default::default(),
            receipts: vec![vec![Some(receipt.clone())]],
            requests: vec![Requests::new(vec![request.clone()])],
            first_block,
        };

        // Define the expected higher ExecutionOutcome after splitting
        let higher_execution_outcome = ExecutionOutcome {
            bundle: Default::default(),
            receipts: vec![vec![Some(receipt.clone())], vec![Some(receipt)]],
            requests: vec![Requests::new(vec![request.clone()]), Requests::new(vec![request])],
            first_block: 124,
        };

        // Assert that the split result matches the expected lower and higher outcomes
        assert_eq!(result.0, Some(lower_execution_outcome));
        assert_eq!(result.1, higher_execution_outcome);

        // Assert that splitting at the first block number returns None for the lower outcome
        assert_eq!(exec_res.clone().split_at(123), (None, exec_res));
    }
}
