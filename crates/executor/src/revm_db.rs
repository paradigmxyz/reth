use std::convert::Infallible;

use reth_interfaces::executor::ExecutorDb;
use reth_primitives::{H160, H256, U256};
use revm::db::Database;

/// Wrapper around ExeuctorDb that implements revm database trait
pub struct Wrapper<'a>(&'a dyn ExecutorDb);

impl<'a> Database for Wrapper<'a> {
    type Error = Infallible;

    fn basic(&mut self, address: H160) -> Result<Option<revm::AccountInfo>, Self::Error> {
        Ok(self.0.basic_account(address).map(|account| revm::AccountInfo {
            balance: account.balance,
            nonce: account.nonce,
            code_hash: account.bytecode_hash,
            code: None,
        }))
    }

    fn code_by_hash(
        &mut self,
        code_hash: reth_primitives::H256,
    ) -> Result<revm::Bytecode, Self::Error> {
        let (bytecode, size) = self.0.bytecode_by_hash(code_hash).unwrap_or_default();
        Ok(unsafe { revm::Bytecode::new_checked(bytecode.0, size, Some(code_hash)) })
    }

    fn storage(
        &mut self,
        address: reth_primitives::H160,
        index: reth_primitives::U256,
    ) -> Result<reth_primitives::U256, Self::Error> {
        todo!()
    }

    fn block_hash(
        &mut self,
        number: reth_primitives::U256,
    ) -> Result<reth_primitives::H256, Self::Error> {
        todo!()
    }
}
