use crate::{
    add_snapshot_mask,
    snapshot::mask::{ColumnMaskOne, ColumnMaskTwo, HeaderMask},
    table::Table,
    CanonicalHeaders, HeaderTD,
};
use reth_primitives::{BlockHash, Header};

add_snapshot_mask!(HeaderMask, Header, 0b001);
add_snapshot_mask!(HeaderMask, BlockHash, 0b100);
add_snapshot_mask!(HeaderMask, <HeaderTD as Table>::Value, 0b010);

add_snapshot_mask!(HeaderMask, <HeaderTD as Table>::Value, <CanonicalHeaders as Table>::Value, 0b110);
add_snapshot_mask!(HeaderMask, Header, BlockHash, 0b101);
