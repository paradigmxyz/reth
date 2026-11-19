use crate::spec::DepositContract;
use alloy_eips::eip6110::MAINNET_DEPOSIT_CONTRACT_ADDRESS;
use alloy_primitives::{b256, BlockNumber, B256};

/// Gas per transaction not creating a contract.
pub const MIN_TRANSACTION_GAS: u64 = 21_000u64;

/// Mainnet prune delete limit.
pub const MAINNET_PRUNE_DELETE_LIMIT: usize = 20000;

/// Deposit contract address: `0x00000000219ab540356cbb839cbe05303d7705fa`
pub(crate) const MAINNET_DEPOSIT_CONTRACT: DepositContract = DepositContract::new(
    MAINNET_DEPOSIT_CONTRACT_ADDRESS,
    11052984,
    b256!("0x649bbc62d0e31342afea4e5cd82d4049e7e1ee912fc0889aa790803be39038c5"),
);

/// Hash of OP Mainnet block 1, present when the pre-Bedrock history was imported.
pub const OVM_HEADER_1_HASH: B256 =
    b256!("0xbee7192e575af30420cae0c7776304ac196077ee72b048970549e4f08e875453");

/// First Bedrock block on OP Mainnet.
pub const OP_MAINNET_BEDROCK_BLOCK: BlockNumber = 105_235_063;
