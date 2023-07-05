use hashbrown::{HashMap, HashSet};
use reth_primitives::{AccessList, AccessListItem, Address, H256};
use revm::{
    interpreter::{opcode, InstructionResult, Interpreter},
    Database, EVMData, Inspector,
};
use std::collections::BTreeSet;

/// An [Inspector] that collects touched accounts and storage slots.
///
/// This can be used to construct an [AccessList] for a transaction via `eth_createAccessList`
#[derive(Default, Debug)]
pub struct AccessListInspector {
    /// All addresses that should be excluded from the final accesslist
    excluded: HashSet<Address>,
    /// All addresses and touched slots
    access_list: HashMap<Address, BTreeSet<H256>>,
}

impl AccessListInspector {
    /// Creates a new inspector instance
    ///
    /// The `access_list` is the provided access list from the call request
    pub fn new(
        access_list: AccessList,
        from: Address,
        to: Address,
        precompiles: Vec<Address>,
    ) -> Self {
        AccessListInspector {
            excluded: [from, to].iter().chain(precompiles.iter()).copied().collect(),
            access_list: access_list
                .0
                .iter()
                .map(|v| (v.address, v.storage_keys.iter().copied().collect()))
                .collect(),
        }
    }

    /// Returns list of addresses and storage keys used by the transaction. It gives you the list of
    /// addresses and storage keys that were touched during execution.
    pub fn into_access_list(self) -> AccessList {
        let items = self.access_list.into_iter().map(|(address, slots)| AccessListItem {
            address,
            storage_keys: slots.into_iter().collect(),
        });
        AccessList(items.collect())
    }

    /// Returns list of addresses and storage keys used by the transaction. It gives you the list of
    /// addresses and storage keys that were touched during execution.
    pub fn access_list(&self) -> AccessList {
        let items = self.access_list.iter().map(|(address, slots)| AccessListItem {
            address: *address,
            storage_keys: slots.iter().copied().collect(),
        });
        AccessList(items.collect())
    }
}

impl<DB> Inspector<DB> for AccessListInspector
where
    DB: Database,
{
    fn step(
        &mut self,
        interpreter: &mut Interpreter,
        _data: &mut EVMData<'_, DB>,
        _is_static: bool,
    ) -> InstructionResult {
        let pc = interpreter.program_counter();
        let op = interpreter.contract.bytecode.bytecode()[pc];

        match op {
            opcode::SLOAD | opcode::SSTORE => {
                if let Ok(slot) = interpreter.stack().peek(0) {
                    let cur_contract = interpreter.contract.address;
                    self.access_list
                        .entry(cur_contract)
                        .or_default()
                        .insert(H256::from(slot.to_be_bytes()));
                }
            }
            opcode::EXTCODECOPY |
            opcode::EXTCODEHASH |
            opcode::EXTCODESIZE |
            opcode::BALANCE |
            opcode::SELFDESTRUCT => {
                if let Ok(slot) = interpreter.stack().peek(0) {
                    let addr: Address = H256::from(slot.to_be_bytes()).into();
                    if !self.excluded.contains(&addr) {
                        self.access_list.entry(addr).or_default();
                    }
                }
            }
            opcode::DELEGATECALL | opcode::CALL | opcode::STATICCALL | opcode::CALLCODE => {
                if let Ok(slot) = interpreter.stack().peek(1) {
                    let addr: Address = H256::from(slot.to_be_bytes()).into();
                    if !self.excluded.contains(&addr) {
                        self.access_list.entry(addr).or_default();
                    }
                }
            }
            _ => (),
        }

        InstructionResult::Continue
    }
}
