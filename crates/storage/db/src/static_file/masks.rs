use super::{ReceiptMask, TransactionMask};
use crate::{
    add_static_file_mask,
    static_file::mask::{ColumnSelectorOne, ColumnSelectorTwo, HeaderMask},
    table::Table,
    HeaderTerminalDifficulties, RawValue, Receipts, Transactions,
};
use reth_primitives::{BlockHash, Header};

// HEADER MASKS
add_static_file_mask!(HeaderMask, Header, 0b001);
add_static_file_mask!(HeaderMask, <HeaderTerminalDifficulties as Table>::Value, 0b010);
add_static_file_mask!(HeaderMask, BlockHash, 0b100);
add_static_file_mask!(HeaderMask, Header, BlockHash, 0b101);
add_static_file_mask!(HeaderMask, <HeaderTerminalDifficulties as Table>::Value, BlockHash, 0b110);

// RECEIPT MASKS
add_static_file_mask!(ReceiptMask, <Receipts as Table>::Value, 0b1);

// TRANSACTION MASKS
add_static_file_mask!(TransactionMask, <Transactions as Table>::Value, 0b1);
add_static_file_mask!(TransactionMask, RawValue<<Transactions as Table>::Value>, 0b1);
