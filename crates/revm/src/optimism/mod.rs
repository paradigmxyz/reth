use reth_interfaces::executor::{self as reth_executor, BlockExecutionError};
use reth_primitives::{Block, Bytes, ChainSpec, Hardfork, TransactionKind, U256};
use revm::{
    primitives::{BedrockSpec, RegolithSpec},
    L1BlockInfo,
};

/// Optimism-specific processor implementation for the `EVMProcessor`
pub mod processor;

/// Extracts the [L1BlockInfo] from the L2 block. The L1 info transaction is always the first
/// transaction in the L2 block.
pub fn extract_l1_info(block: &Block) -> Result<L1BlockInfo, BlockExecutionError> {
    let l1_info_tx_data = block
        .body
        .iter()
        .find(|tx| matches!(tx.kind(), TransactionKind::Call(to) if to == &revm::optimism::L1_BLOCK_CONTRACT))
        .ok_or(reth_executor::BlockExecutionError::OptimismBlockExecution(
            reth_executor::OptimismBlockExecutionError::L1BlockInfoError {
                message: "could not find l1 block info tx in the L2 block".to_string(),
        }))
        .and_then(|tx| {
            tx.input().get(4..).ok_or(reth_executor::BlockExecutionError::OptimismBlockExecution(
                reth_executor::OptimismBlockExecutionError::L1BlockInfoError {
                message: "could not get l1 block info tx calldata bytes".to_string(),
            }))
        })?;

    parse_l1_info_tx(l1_info_tx_data)
}

/// Parses the calldata of the [L1BlockInfo] transaction.
pub fn parse_l1_info_tx(data: &[u8]) -> Result<L1BlockInfo, BlockExecutionError> {
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

    Ok(L1BlockInfo { l1_base_fee, l1_fee_overhead, l1_fee_scalar })
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
        input: &Bytes,
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
        input: &Bytes,
    ) -> Result<U256, BlockExecutionError>;
}

impl RethL1BlockInfo for L1BlockInfo {
    fn l1_tx_data_fee(
        &self,
        chain_spec: &ChainSpec,
        timestamp: u64,
        input: &Bytes,
        is_deposit: bool,
    ) -> Result<U256, BlockExecutionError> {
        if is_deposit {
            return Ok(U256::ZERO)
        }

        if chain_spec.is_fork_active_at_timestamp(Hardfork::Regolith, timestamp) {
            Ok(self.calculate_tx_l1_cost::<RegolithSpec>(input))
        } else if chain_spec.is_fork_active_at_timestamp(Hardfork::Bedrock, timestamp) {
            Ok(self.calculate_tx_l1_cost::<BedrockSpec>(input))
        } else {
            Err(reth_executor::BlockExecutionError::OptimismBlockExecution(
                reth_executor::OptimismBlockExecutionError::L1BlockInfoError {
                    message: "Optimism hardforks are not active".to_string(),
                },
            ))
        }
    }

    fn l1_data_gas(
        &self,
        chain_spec: &ChainSpec,
        timestamp: u64,
        input: &Bytes,
    ) -> Result<U256, BlockExecutionError> {
        if chain_spec.is_fork_active_at_timestamp(Hardfork::Regolith, timestamp) {
            Ok(self.data_gas::<RegolithSpec>(input))
        } else if chain_spec.is_fork_active_at_timestamp(Hardfork::Bedrock, timestamp) {
            Ok(self.data_gas::<BedrockSpec>(input))
        } else {
            Err(reth_executor::BlockExecutionError::OptimismBlockExecution(
                reth_executor::OptimismBlockExecutionError::L1BlockInfoError {
                    message: "Optimism hardforks are not active".to_string(),
                },
            ))
        }
    }
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
        assert_eq!(l1_info.l1_fee_overhead, U256::from(2100));
        assert_eq!(l1_info.l1_fee_scalar, U256::from(1_000_000));
    }
}
