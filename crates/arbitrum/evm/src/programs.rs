#![allow(unused)]

use revm::Database;
use crate::storage::Storage;

pub struct Programs<D> {
    backing_storage: Storage<D>,
    arbos_version: u64,
}

impl<D: Database> Programs<D> {
    pub fn open(arbos_version: u64, sto: Storage<D>) -> Self {
        Self {
            backing_storage: sto,
            arbos_version,
        }
    }

    pub fn initialize(&self) -> Result<(), ()> {
        Ok(())
    }
}
