//! Address tracing inspector
//!
//! When a transaction executes, an address may appear in several places.
//! This tracer looks for potential addresses and records them.
//!
//! This is used to construct and index associated with the `address` namespace.
//! The index is a map of address -> transactions.
//!
//! The appearance specification is located here:
//! <https://github.com/ethereum/execution-apis/pull/456>. In addition to obvious
//! places an address may appear (address fields in opcodes), "possible addresses"
//! are also searched for. These are found by looking in call data and return data
//! and applying heuristics as outlined in the spec.
use std::collections::HashSet;

use reth_primitives::{bytes::Bytes, Address};
use revm::{
    interpreter::{CallInputs, CreateInputs, Gas, InstructionResult},
    primitives::{db::Database, B160},
    EVMData, Inspector,
};

/// Address tracing inspector that records all address that appear during execution.
#[derive(Clone, Default)]
pub struct AddressInspector {
    /// Unique addresses observed in the transaction
    inner: HashSet<Address>,
}

impl AddressInspector {
    /// Returns the addresses that appeared in the transaction
    pub fn inner(&self) -> &HashSet<Address> {
        &self.inner
    }

    /// Detects addresses in bytes and adds them to the current set.
    fn extract_addresses(&mut self, bytes: &[u8]) {
        if bytes.len() < 20 {
            return
        }

        if let Some(addresses) = bytes_to_addresses(&bytes) {
            for address in addresses {
                self.inner.insert(address);
            }
        }
    }
}

impl<DB: Database> Inspector<DB> for AddressInspector {
    fn call_end(
        &mut self,
        _data: &mut EVMData<'_, DB>,
        _inputs: &CallInputs,
        remaining_gas: Gas,
        ret: InstructionResult,
        out: Bytes,
        _is_static: bool,
    ) -> (InstructionResult, Gas, Bytes) {
        self.extract_addresses(&out);
        (ret, remaining_gas, out)
    }

    fn create_end(
        &mut self,
        _data: &mut EVMData<'_, DB>,
        _inputs: &CreateInputs,
        ret: InstructionResult,
        address: Option<B160>,
        remaining_gas: Gas,
        out: Bytes,
    ) -> (InstructionResult, Option<B160>, Gas, Bytes) {
        if let Some(addr) = address {
            self.inner.insert(addr);
        }
        self.extract_addresses(&out);
        (ret, address, remaining_gas, out)
    }

    fn call(
        &mut self,
        _data: &mut EVMData<'_, DB>,
        inputs: &mut CallInputs,
        _is_static: bool,
    ) -> (InstructionResult, Gas, Bytes) {
        self.inner.insert(inputs.contract);
        self.inner.insert(inputs.context.address);
        self.inner.insert(inputs.context.caller);
        self.inner.insert(inputs.context.code_address);
        self.extract_addresses(&inputs.input);
        (InstructionResult::Continue, Gas::new(0), Bytes::new())
    }

    fn create(
        &mut self,
        _data: &mut EVMData<'_, DB>,
        inputs: &mut CreateInputs,
    ) -> (InstructionResult, Option<B160>, Gas, Bytes) {
        self.inner.insert(inputs.caller);
        (InstructionResult::Continue, None, Gas::new(0), Bytes::new())
    }

    fn selfdestruct(&mut self, contract: B160, target: B160) {
        self.inner.insert(contract);
        self.inner.insert(target);
    }
}

/// Searches for and returns addresses that are present in bytes.
///
/// See also: <https://github.com/ethereum/execution-apis/pull/456> for the algorithm specification.
fn bytes_to_addresses(_candidate: &[u8]) -> Option<Vec<Address>> {
    // todo
    None
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn count_addresses() {
        use revm::primitives::hex_literal;
        // Test from: https://github.com/bluealloy/revm/issues/277
        let mut evm = revm::new();
        let mut database = revm::InMemoryDB::default();
        let code: revm::primitives::Bytes = hex_literal::hex!("5b597fb075978b6c412c64d169d56d839a8fe01b3f4607ed603b2c78917ce8be1430fe6101e8527ffe64706ecad72a2f5c97a95e006e279dc57081902029ce96af7edae5de116fec610208527f9fc1ef09d4dd80683858ae3ea18869fe789ddc365d8d9d800e26c9872bac5e5b6102285260276102485360d461024953601661024a53600e61024b53607d61024c53600961024d53600b61024e5360b761024f5360596102505360796102515360a061025253607261025353603a6102545360fb61025553601261025653602861025753600761025853606f61025953601761025a53606161025b53606061025c5360a661025d53602b61025e53608961025f53607a61026053606461026153608c6102625360806102635360d56102645360826102655360ae61026653607f6101e8610146610220677a814b184591c555735fdcca53617f4d2b9134b29090c87d01058e27e962047654f259595947443b1b816b65cdb6277f4b59c10a36f4e7b8658f5a5e6f5561").to_vec().into();

        let acc_info = revm::primitives::AccountInfo {
            balance: "0x100c5d668240db8e00".parse().unwrap(),
            code_hash: revm::primitives::keccak256(&code),
            code: Some(revm::primitives::Bytecode::new_raw(code.clone())),
            nonce: "1".parse().unwrap(),
        };
        let caller = hex_literal::hex!("5fdcca53617f4d2b9134b29090c87d01058e27e0");
        let callee = hex_literal::hex!("5fdcca53617f4d2b9134b29090c87d01058e27e9");

        database.insert_account_info(revm::primitives::B160(callee), acc_info);
        evm.database(database);
        evm.env.tx.caller = revm::primitives::B160(caller);
        evm.env.tx.transact_to = revm::primitives::TransactTo::Call(revm::primitives::B160(callee));
        evm.env.tx.data = revm::primitives::Bytes::new();
        evm.env.tx.value = revm::primitives::U256::ZERO;
        let mut inspector = AddressInspector::default();
        let result = evm.inspect_commit(&mut inspector).unwrap();
        let addresses = inspector.inner();
        assert_eq!(addresses.len(), 2);
        assert!(addresses.contains(&B160(caller)));
        assert!(addresses.contains(&B160(callee)));
        // Note: Addresses from logs are retrieved from the result of the inspector
        assert!(result.logs().is_empty());
    }
}
