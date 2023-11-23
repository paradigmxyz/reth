use alloy_primitives::{Address, B256};
use reth_rpc_types::{AccessList, AccessListItem};
use revm::{
    interpreter::{opcode, Interpreter},
    Database, EVMData, Inspector,
};
use std::collections::{BTreeSet, HashMap, HashSet};

/// An [Inspector] that collects touched accounts and storage slots.
///
/// This can be used to construct an [AccessList] for a transaction via `eth_createAccessList`
#[derive(Default, Debug)]
pub struct AccessListInspector {
    /// All addresses that should be excluded from the final accesslist
    excluded: HashSet<Address>,
    /// All addresses and touched slots
    access_list: HashMap<Address, BTreeSet<B256>>,
}

impl AccessListInspector {
    /// Creates a new inspector instance
    ///
    /// The `access_list` is the provided access list from the call request
    pub fn new(
        access_list: AccessList,
        from: Address,
        to: Address,
        precompiles: impl IntoIterator<Item = Address>,
    ) -> Self {
        AccessListInspector {
            excluded: [from, to].into_iter().chain(precompiles).collect(),
            access_list: access_list
                .0
                .into_iter()
                .map(|v| (v.address, v.storage_keys.into_iter().collect()))
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
    fn step(&mut self, interpreter: &mut Interpreter<'_>, _data: &mut EVMData<'_, DB>) {
        match interpreter.current_opcode() {
            opcode::SLOAD | opcode::SSTORE => {
                if let Ok(slot) = interpreter.stack().peek(0) {
                    let cur_contract = interpreter.contract.address;
                    self.access_list
                        .entry(cur_contract)
                        .or_default()
                        .insert(B256::from(slot.to_be_bytes()));
                }
            }
            opcode::EXTCODECOPY |
            opcode::EXTCODEHASH |
            opcode::EXTCODESIZE |
            opcode::BALANCE |
            opcode::SELFDESTRUCT => {
                if let Ok(slot) = interpreter.stack().peek(0) {
                    let addr = Address::from_word(B256::from(slot.to_be_bytes()));
                    if !self.excluded.contains(&addr) {
                        self.access_list.entry(addr).or_default();
                    }
                }
            }
            opcode::DELEGATECALL | opcode::CALL | opcode::STATICCALL | opcode::CALLCODE => {
                if let Ok(slot) = interpreter.stack().peek(1) {
                    let addr = Address::from_word(B256::from(slot.to_be_bytes()));
                    if !self.excluded.contains(&addr) {
                        self.access_list.entry(addr).or_default();
                    }
                }
            }
            _ => (),
        }
    }
}
