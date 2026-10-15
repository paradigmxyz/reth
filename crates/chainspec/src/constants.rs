use crate::spec::DepositContract;
use alloy_eips::{eip6110::MAINNET_DEPOSIT_CONTRACT_ADDRESS, eip7840::BlobParams};
use alloy_primitives::b256;

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

/// EIP-8198 blob schedule for the Bogota fork.
pub(crate) const BOGOTA_BLOB_PARAMS: BlobParams = BlobParams {
    target_blob_count: 12,
    max_blob_count: 18,
    update_fraction: 12_018_519,
    ..BlobParams::osaka()
};
