#![allow(missing_docs)]

use reth_db::{
    table::{Table, TableInfo},
    TableSet,
};

#[derive(Debug)]
pub struct TxTable; // TxHash -> Vec<InternalTransaction>
#[derive(Debug)]
pub struct BlockTable; // BlockHash -> Vec<TxHash>

impl Table for BlockTable {
    const NAME: &'static str = "BlockTable";
    const DUPSORT: bool = false;
    type Key = Vec<u8>;
    type Value = Vec<u8>;
}

impl TableInfo for BlockTable {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn is_dupsort(&self) -> bool {
        Self::DUPSORT
    }
}

impl Table for TxTable {
    const NAME: &'static str = "TxTable";
    const DUPSORT: bool = false;
    type Key = Vec<u8>;
    type Value = Vec<u8>;
}

impl TableInfo for TxTable {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn is_dupsort(&self) -> bool {
        Self::DUPSORT
    }
}

#[derive(Debug)]
pub struct DBTables;

impl TableSet for DBTables {
    fn tables() -> Box<dyn Iterator<Item = Box<dyn TableInfo>>> {
        let v: Vec<Box<dyn TableInfo>> = vec![Box::new(BlockTable), Box::new(TxTable)];

        Box::new(v.into_iter())
    }
}
