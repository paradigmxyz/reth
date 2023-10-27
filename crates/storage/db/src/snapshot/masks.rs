use super::{ReceiptMask, TransactionMask};
use crate::{
    add_snapshot_mask,
    snapshot::mask::{ColumnSelectorOne, ColumnSelectorTwo, HeaderMask},
    table::Table,
    CanonicalHeaders, HeaderTD, Receipts, Transactions,
};
use reth_primitives::{BlockHash, Header};

// HEADER MASKS

add_snapshot_mask!(HeaderMask, Header, 0b001);
add_snapshot_mask!(HeaderMask, <HeaderTD as Table>::Value, 0b010);
add_snapshot_mask!(HeaderMask, BlockHash, 0b100);

add_snapshot_mask!(HeaderMask, Header, BlockHash, 0b101);
add_snapshot_mask!(
    HeaderMask,
    <HeaderTD as Table>::Value,
    <CanonicalHeaders as Table>::Value,
    0b110
);

// RECEIPT MASKS
add_snapshot_mask!(ReceiptMask, <Receipts as Table>::Value, 0b1);

// TRANSACTION MASKS
add_snapshot_mask!(TransactionMask, <Transactions as Table>::Value, 0b1);
