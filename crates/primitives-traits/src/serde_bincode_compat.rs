//! Serde helpers for bincode-compatible serialization.
//!
//! This module previously contained the `SerdeBincodeCompat` trait which has been removed.
//! Block types now use RLP encoding for serialization in contexts that require bincode
//! compatibility (e.g., ExEx WAL, header stage ETL).
