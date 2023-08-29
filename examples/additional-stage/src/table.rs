//! Example custom table implementation.

use reth_db::{table, table::Table, tables, TableMetadata, TableType};
use reth_primitives::{Address, BlockNumber};

// Usage: (TableName) KeyType | ValueType
table!(
    /// Stores the last block an address appears in (for demonstration purposes).
    ( MyTable ) Address | BlockNumber
);

// Sets up a new NonCoreTable enum containing only new tables (no core reth tables).
tables!(NonCoreTable, 1, [(MyTable, TableType::Table)]);
