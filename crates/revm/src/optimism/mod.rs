use reth_interfaces::{
    executor::{self as reth_executor, BlockExecutionError},
    RethError,
};
use reth_primitives::{address, b256, hex, Address, Block, Bytes, ChainSpec, Hardfork, B256, U256};
use revm::{
    primitives::{Bytecode, HashMap, SpecId},
    DatabaseCommit, L1BlockInfo,
};
use std::sync::Arc;
use tracing::trace;

/// Optimism-specific processor implementation for the `EVMProcessor`
pub mod processor;

/// The address of the create2 deployer
const CREATE_2_DEPLOYER_ADDR: Address = address!("13b0D85CcB8bf860b6b79AF3029fCA081AE9beF2");
/// The codehash of the create2 deployer contract.
const CREATE_2_DEPLOYER_CODEHASH: B256 =
    b256!("b0550b5b431e30d38000efb7107aaa0ade03d48a7198a140edda9d27134468b2");
/// The raw bytecode of the create2 deployer contract.
const CREATE_2_DEPLOYER_BYTECODE: [u8; 1584] = hex!("6080604052600436106100435760003560e01c8063076c37b21461004f578063481286e61461007157806356299481146100ba57806366cfa057146100da57600080fd5b3661004a57005b600080fd5b34801561005b57600080fd5b5061006f61006a366004610327565b6100fa565b005b34801561007d57600080fd5b5061009161008c366004610327565b61014a565b60405173ffffffffffffffffffffffffffffffffffffffff909116815260200160405180910390f35b3480156100c657600080fd5b506100916100d5366004610349565b61015d565b3480156100e657600080fd5b5061006f6100f53660046103ca565b610172565b61014582826040518060200161010f9061031a565b7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe082820381018352601f90910116604052610183565b505050565b600061015683836102e7565b9392505050565b600061016a8484846102f0565b949350505050565b61017d838383610183565b50505050565b6000834710156101f4576040517f08c379a000000000000000000000000000000000000000000000000000000000815260206004820152601d60248201527f437265617465323a20696e73756666696369656e742062616c616e636500000060448201526064015b60405180910390fd5b815160000361025f576040517f08c379a000000000000000000000000000000000000000000000000000000000815260206004820181905260248201527f437265617465323a2062797465636f6465206c656e677468206973207a65726f60448201526064016101eb565b8282516020840186f5905073ffffffffffffffffffffffffffffffffffffffff8116610156576040517f08c379a000000000000000000000000000000000000000000000000000000000815260206004820152601960248201527f437265617465323a204661696c6564206f6e206465706c6f790000000000000060448201526064016101eb565b60006101568383305b6000604051836040820152846020820152828152600b8101905060ff815360559020949350505050565b61014e806104ad83390190565b6000806040838503121561033a57600080fd5b50508035926020909101359150565b60008060006060848603121561035e57600080fd5b8335925060208401359150604084013573ffffffffffffffffffffffffffffffffffffffff8116811461039057600080fd5b809150509250925092565b7f4e487b7100000000000000000000000000000000000000000000000000000000600052604160045260246000fd5b6000806000606084860312156103df57600080fd5b8335925060208401359150604084013567ffffffffffffffff8082111561040557600080fd5b818601915086601f83011261041957600080fd5b81358181111561042b5761042b61039b565b604051601f82017fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe0908116603f011681019083821181831017156104715761047161039b565b8160405282815289602084870101111561048a57600080fd5b826020860160208301376000602084830101528095505050505050925092509256fe608060405234801561001057600080fd5b5061012e806100206000396000f3fe6080604052348015600f57600080fd5b506004361060285760003560e01c8063249cb3fa14602d575b600080fd5b603c603836600460b1565b604e565b60405190815260200160405180910390f35b60008281526020818152604080832073ffffffffffffffffffffffffffffffffffffffff8516845290915281205460ff16608857600060aa565b7fa2ef4600d742022d532d4747cb3547474667d6f13804902513b2ec01c848f4b45b9392505050565b6000806040838503121560c357600080fd5b82359150602083013573ffffffffffffffffffffffffffffffffffffffff8116811460ed57600080fd5b80915050925092905056fea26469706673582212205ffd4e6cede7d06a5daf93d48d0541fc68189eeb16608c1999a82063b666eb1164736f6c63430008130033a2646970667358221220fdc4a0fe96e3b21c108ca155438d37c9143fb01278a3c1d274948bad89c564ba64736f6c63430008130033");

/// The function selector of the "setL1BlockValuesEcotone" function in the L1Block contract.
const L1_BLOCK_ECOTONE_SELECTOR: [u8; 4] = hex!("440a5e20");

/// Extracts the [L1BlockInfo] from the L2 block. The L1 info transaction is always the first
/// transaction in the L2 block.
pub fn extract_l1_info(block: &Block) -> Result<L1BlockInfo, BlockExecutionError> {
    let l1_info_tx_data = block
        .body
        .first()
        .ok_or(reth_executor::BlockExecutionError::OptimismBlockExecution(
            reth_executor::OptimismBlockExecutionError::L1BlockInfoError {
                message: "could not find l1 block info tx in the L2 block".to_string(),
            },
        ))
        .map(|tx| tx.input())?;

    // If the first 4 bytes of the calldata are the L1BlockInfoEcotone selector, then we parse the
    // calldata as an Ecotone hardfork L1BlockInfo transaction. Otherwise, we parse it as a
    // Bedrock hardfork L1BlockInfo transaction.
    if l1_info_tx_data[0..4] == L1_BLOCK_ECOTONE_SELECTOR {
        parse_l1_info_tx_ecotone(l1_info_tx_data[4..].as_ref())
    } else {
        parse_l1_info_tx_bedrock(l1_info_tx_data[4..].as_ref())
    }
}

/// Parses the calldata of the [L1BlockInfo] transaction pre-Ecotone hardfork.
pub fn parse_l1_info_tx_bedrock(data: &[u8]) -> Result<L1BlockInfo, BlockExecutionError> {
    // The setL1BlockValues tx calldata must be exactly 260 bytes long, considering that
    // we already removed the first 4 bytes (the function selector). Detailed breakdown:
    //   32 bytes for the block number
    // + 32 bytes for the block timestamp
    // + 32 bytes for the base fee
    // + 32 bytes for the block hash
    // + 32 bytes for the block sequence number
    // + 32 bytes for the batcher hash
    // + 32 bytes for the fee overhead
    // + 32 bytes for the fee scalar
    if data.len() != 256 {
        return Err(reth_executor::BlockExecutionError::OptimismBlockExecution(
            reth_executor::OptimismBlockExecutionError::L1BlockInfoError {
                message: "unexpected l1 block info tx calldata length found".to_string(),
            },
        ))
    }

    let l1_base_fee = U256::try_from_be_slice(&data[64..96]).ok_or(
        reth_executor::BlockExecutionError::OptimismBlockExecution(
            reth_executor::OptimismBlockExecutionError::L1BlockInfoError {
                message: "could not convert l1 base fee".to_string(),
            },
        ),
    )?;
    let l1_fee_overhead = U256::try_from_be_slice(&data[192..224]).ok_or(
        reth_executor::BlockExecutionError::OptimismBlockExecution(
            reth_executor::OptimismBlockExecutionError::L1BlockInfoError {
                message: "could not convert l1 fee overhead".to_string(),
            },
        ),
    )?;
    let l1_fee_scalar = U256::try_from_be_slice(&data[224..256]).ok_or(
        reth_executor::BlockExecutionError::OptimismBlockExecution(
            reth_executor::OptimismBlockExecutionError::L1BlockInfoError {
                message: "could not convert l1 fee scalar".to_string(),
            },
        ),
    )?;

    let mut l1block = L1BlockInfo::default();
    l1block.l1_base_fee = l1_base_fee;
    l1block.l1_fee_overhead = Some(l1_fee_overhead);
    l1block.l1_base_fee_scalar = l1_fee_scalar;

    Ok(l1block)
}

/// Parses the calldata of the [L1BlockInfo] transaction post-Ecotone hardfork.
pub fn parse_l1_info_tx_ecotone(data: &[u8]) -> Result<L1BlockInfo, BlockExecutionError> {
    // The setL1BlockValuesEcotone tx calldata must be exactly 160 bytes long, considering that
    // we already removed the first 4 bytes (the function selector). Detailed breakdown:
    //   8 bytes for the block sequence number
    // + 4 bytes for the blob base fee scalar
    // + 4 bytes for the base fee scalar
    // + 8 bytes for the block number
    // + 8 bytes for the block timestamp
    // + 32 bytes for the base fee
    // + 32 bytes for the blob base fee
    // + 32 bytes for the block hash
    // + 32 bytes for the batcher hash
    if data.len() != 160 {
        return Err(reth_executor::BlockExecutionError::OptimismBlockExecution(
            reth_executor::OptimismBlockExecutionError::L1BlockInfoError {
                message: "unexpected l1 block info tx calldata length found".to_string(),
            },
        ))
    }

    let l1_blob_base_fee_scalar = U256::try_from_be_slice(&data[8..12]).ok_or(
        reth_executor::BlockExecutionError::OptimismBlockExecution(
            reth_executor::OptimismBlockExecutionError::L1BlockInfoError {
                message: "could not convert l1 blob base fee scalar".to_string(),
            },
        ),
    )?;
    let l1_base_fee_scalar = U256::try_from_be_slice(&data[12..16]).ok_or(
        reth_executor::BlockExecutionError::OptimismBlockExecution(
            reth_executor::OptimismBlockExecutionError::L1BlockInfoError {
                message: "could not convert l1 base fee scalar".to_string(),
            },
        ),
    )?;
    let l1_base_fee = U256::try_from_be_slice(&data[32..64]).ok_or(
        reth_executor::BlockExecutionError::OptimismBlockExecution(
            reth_executor::OptimismBlockExecutionError::L1BlockInfoError {
                message: "could not convert l1 blob base fee".to_string(),
            },
        ),
    )?;
    let l1_blob_base_fee = U256::try_from_be_slice(&data[64..96]).ok_or(
        reth_executor::BlockExecutionError::OptimismBlockExecution(
            reth_executor::OptimismBlockExecutionError::L1BlockInfoError {
                message: "could not convert l1 blob base fee".to_string(),
            },
        ),
    )?;

    let mut l1block = L1BlockInfo::default();
    l1block.l1_base_fee = l1_base_fee;
    l1block.l1_base_fee_scalar = l1_base_fee_scalar;
    l1block.l1_blob_base_fee = Some(l1_blob_base_fee);
    l1block.l1_blob_base_fee_scalar = Some(l1_blob_base_fee_scalar);

    Ok(l1block)
}

/// An extension trait for [L1BlockInfo] that allows us to calculate the L1 cost of a transaction
/// based off of the [ChainSpec]'s activated hardfork.
pub trait RethL1BlockInfo {
    /// Forwards an L1 transaction calculation to revm and returns the gas cost.
    ///
    /// ### Takes
    /// - `chain_spec`: The [ChainSpec] for the node.
    /// - `timestamp`: The timestamp of the current block.
    /// - `input`: The calldata of the transaction.
    /// - `is_deposit`: Whether or not the transaction is a deposit.
    fn l1_tx_data_fee(
        &self,
        chain_spec: &ChainSpec,
        timestamp: u64,
        input: &[u8],
        is_deposit: bool,
    ) -> Result<U256, BlockExecutionError>;

    /// Computes the data gas cost for an L2 transaction.
    ///
    /// ### Takes
    /// - `chain_spec`: The [ChainSpec] for the node.
    /// - `timestamp`: The timestamp of the current block.
    /// - `input`: The calldata of the transaction.
    fn l1_data_gas(
        &self,
        chain_spec: &ChainSpec,
        timestamp: u64,
        input: &[u8],
    ) -> Result<U256, BlockExecutionError>;
}

impl RethL1BlockInfo for L1BlockInfo {
    fn l1_tx_data_fee(
        &self,
        chain_spec: &ChainSpec,
        timestamp: u64,
        input: &[u8],
        is_deposit: bool,
    ) -> Result<U256, BlockExecutionError> {
        if is_deposit {
            return Ok(U256::ZERO)
        }

        let spec_id = if chain_spec.is_fork_active_at_timestamp(Hardfork::Regolith, timestamp) {
            SpecId::REGOLITH
        } else if chain_spec.is_fork_active_at_timestamp(Hardfork::Bedrock, timestamp) {
            SpecId::BEDROCK
        } else {
            return Err(reth_executor::BlockExecutionError::OptimismBlockExecution(
                reth_executor::OptimismBlockExecutionError::L1BlockInfoError {
                    message: "Optimism hardforks are not active".to_string(),
                },
            ))
        };
        Ok(self.calculate_tx_l1_cost(input, spec_id))
    }

    fn l1_data_gas(
        &self,
        chain_spec: &ChainSpec,
        timestamp: u64,
        input: &[u8],
    ) -> Result<U256, BlockExecutionError> {
        let spec_id = if chain_spec.is_fork_active_at_timestamp(Hardfork::Regolith, timestamp) {
            SpecId::REGOLITH
        } else if chain_spec.is_fork_active_at_timestamp(Hardfork::Bedrock, timestamp) {
            SpecId::BEDROCK
        } else {
            return Err(reth_executor::BlockExecutionError::OptimismBlockExecution(
                reth_executor::OptimismBlockExecutionError::L1BlockInfoError {
                    message: "Optimism hardforks are not active".to_string(),
                },
            ))
        };
        Ok(self.data_gas(input, spec_id))
    }
}

/// The Canyon hardfork issues an irregular state transition that force-deploys the create2
/// deployer contract. This is done by directly setting the code of the create2 deployer account
/// prior to executing any transactions on the timestamp activation of the fork.
pub fn ensure_create2_deployer<DB>(
    chain_spec: Arc<ChainSpec>,
    timestamp: u64,
    db: &mut revm::State<DB>,
) -> Result<(), RethError>
where
    DB: revm::Database,
{
    // If the canyon hardfork is active at the current timestamp, and it was not active at the
    // previous block timestamp (heuristically, block time is not perfectly constant at 2s), and the
    // chain is an optimism chain, then we need to force-deploy the create2 deployer contract.
    if chain_spec.is_optimism() &&
        chain_spec.is_fork_active_at_timestamp(Hardfork::Canyon, timestamp) &&
        !chain_spec.is_fork_active_at_timestamp(Hardfork::Canyon, timestamp - 2)
    {
        trace!(target: "evm", "Forcing create2 deployer contract deployment on Canyon transition");

        // Load the create2 deployer account from the cache.
        let acc = db
            .load_cache_account(CREATE_2_DEPLOYER_ADDR)
            .map_err(|_| RethError::Custom("Failed to load account".to_string()))?;

        // Update the account info with the create2 deployer codehash and bytecode.
        let mut acc_info = acc.account_info().unwrap_or_default();
        acc_info.code_hash = CREATE_2_DEPLOYER_CODEHASH;
        acc_info.code = Some(Bytecode::new_raw(Bytes::from_static(&CREATE_2_DEPLOYER_BYTECODE)));

        // Convert the cache account back into a revm account and mark it as touched.
        let mut revm_acc: revm::primitives::Account = acc_info.into();
        revm_acc.mark_touch();

        // Commit the create2 deployer account to the database.
        db.commit(HashMap::from([(CREATE_2_DEPLOYER_ADDR, revm_acc)]));
        return Ok(())
    }

    Ok(())
}

#[cfg(test)]
mod test_l1_fee {
    #[test]
    fn sanity_l1_block() {
        use super::*;
        use reth_primitives::{hex_literal::hex, Bytes, Header, TransactionSigned};

        let bytes = Bytes::from_static(&hex!("7ef9015aa044bae9d41b8380d781187b426c6fe43df5fb2fb57bd4466ef6a701e1f01e015694deaddeaddeaddeaddeaddeaddeaddeaddead000194420000000000000000000000000000000000001580808408f0d18001b90104015d8eb900000000000000000000000000000000000000000000000000000000008057650000000000000000000000000000000000000000000000000000000063d96d10000000000000000000000000000000000000000000000000000000000009f35273d89754a1e0387b89520d989d3be9c37c1f32495a88faf1ea05c61121ab0d1900000000000000000000000000000000000000000000000000000000000000010000000000000000000000002d679b567db6187c0c8323fa982cfb88b74dbcc7000000000000000000000000000000000000000000000000000000000000083400000000000000000000000000000000000000000000000000000000000f4240"));
        let l1_info_tx = TransactionSigned::decode_enveloped(&mut bytes.as_ref()).unwrap();
        let mock_block = Block {
            header: Header::default(),
            body: vec![l1_info_tx],
            ommers: Vec::default(),
            withdrawals: None,
        };

        let l1_info: L1BlockInfo = super::extract_l1_info(&mock_block).unwrap();
        assert_eq!(l1_info.l1_base_fee, U256::from(652_114));
        assert_eq!(l1_info.l1_fee_overhead, Some(U256::from(2100)));
        assert_eq!(l1_info.l1_base_fee_scalar, U256::from(1_000_000));
        assert_eq!(l1_info.l1_blob_base_fee, None);
        assert_eq!(l1_info.l1_blob_base_fee_scalar, None);
    }

    #[test]
    fn sanity_l1_block_ecotone() {
        use super::*;
        use reth_primitives::{hex_literal::hex, Bytes, Header, TransactionSigned};

        let bytes = Bytes::from_static(&hex!("7ef8f8a0b84fa363879a2159e341c50a32da3ea0d21765b7bd43db37f2e5e04e8848b1ee94deaddeaddeaddeaddeaddeaddeaddeaddead00019442000000000000000000000000000000000000158080830f424080b8a4440a5e20000f42400000000000000000000000040000000065c41f680000000000a03f6b00000000000000000000000000000000000000000000000000000000000000080000000000000000000000000000000000000000000000000000000535f4d983dea59eac60478a64ecfdcde8571e611404295350de7ed4ccb404296c1a84ab7a00000000000000000000000073b4168cc87f35cc239200a20eb841cded23493b"));
        let l1_info_tx = TransactionSigned::decode_enveloped(&mut bytes.as_ref()).unwrap();
        let mock_block = Block {
            header: Header::default(),
            body: vec![l1_info_tx],
            ommers: Vec::default(),
            withdrawals: None,
        };

        let l1_info: L1BlockInfo = super::extract_l1_info(&mock_block).unwrap();
        assert_eq!(l1_info.l1_base_fee, U256::from(8));
        assert_eq!(l1_info.l1_base_fee_scalar, U256::from(4));
        assert_eq!(l1_info.l1_blob_base_fee, Some(U256::from(22_380_075_395u64)));
        assert_eq!(l1_info.l1_blob_base_fee_scalar, Some(U256::from(0)));
        assert_eq!(l1_info.l1_fee_overhead, None);
    }
}
