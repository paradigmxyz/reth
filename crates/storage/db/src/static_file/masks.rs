use crate::{
    add_static_file_mask,
    static_file::mask::{ColumnSelectorOne, ColumnSelectorTwo},
    HeaderTerminalDifficulties,
};
use alloy_primitives::{Address, BlockHash};
use reth_db_api::{models::StorageBeforeTx, table::Table, AccountChangeSets};

// HEADER MASKS
add_static_file_mask! {
    #[doc = "Mask for selecting a single header from Headers static file segment"]
    HeaderMask<H>, H, 0b001
}
add_static_file_mask! {
    #[doc = "Mask for selecting a total difficulty value from Headers static file segment"]
    TotalDifficultyMask, <HeaderTerminalDifficulties as Table>::Value, 0b010
}
add_static_file_mask! {
    #[doc = "Mask for selecting a block hash value from Headers static file segment"]
    BlockHashMask, BlockHash, 0b100
}
add_static_file_mask! {
    #[doc = "Mask for selecting a header along with block hash from Headers static file segment"]
    HeaderWithHashMask<H>, H, BlockHash, 0b101
}
add_static_file_mask! {
    #[doc = "Mask for selecting a total difficulty along with block hash from Headers static file segment"]
    TDWithHashMask,
    <HeaderTerminalDifficulties as Table>::Value,
    BlockHash,
    0b110
}

// RECEIPT MASKS
add_static_file_mask! {
    #[doc = "Mask for selecting a single receipt from `Receipts` static file segment"]
    ReceiptMask<R>, R, 0b1
}

// TRANSACTION MASKS
add_static_file_mask! {
    #[doc = "Mask for selecting a single transaction from `Transactions` static file segment"]
    TransactionMask<T>, T, 0b1
}

// TRANSACTION SENDER MASKS
add_static_file_mask! {
    #[doc = "Mask for selecting a single transaction sender from `TransactionSenders` static file segment"]
    TransactionSenderMask, Address, 0b1
}

// ACCOUNT CHANGESET MASKS
add_static_file_mask! {
    #[doc = "Mask for selecting a single changeset from `AccountChangesets` static file segment"]
    AccountChangesetMask, <AccountChangeSets as Table>::Value, 0b1
}

// STORAGE CHANGESET MASKS
add_static_file_mask! {
    #[doc = "Mask for selecting a single changeset from `StorageChangesets` static file segment"]
    StorageChangesetMask, StorageBeforeTx, 0b1
}
