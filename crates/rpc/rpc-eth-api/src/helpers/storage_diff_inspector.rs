use alloy_primitives::{Address, StorageKey, StorageValue, B256, U256};
use revm::{
    context::ContextTr,
    interpreter::{bytecode::opcode::OpCode, Interpreter, InterpreterTypes},
};
use revm_inspectors::inspector::Inspector;
use std::collections::BTreeMap;

/// Captures per-transaction storage diffs by watching `SSTORE` instructions during replay.
#[derive(Default, Debug)]
pub struct StorageDiffInspector {
    /// Diff recorded while the current transaction executes.
    current_tx: BTreeMap<Address, BTreeMap<StorageKey, StorageValue>>,
    /// Diffs for each transaction in order of execution.
    tx_diffs: Vec<BTreeMap<Address, BTreeMap<StorageKey, StorageValue>>>,
}

impl StorageDiffInspector {
    /// Creates a new inspector instance.
    pub const fn new() -> Self {
        Self { current_tx: BTreeMap::new(), tx_diffs: Vec::new() }
    }

    /// Marks the end of the current transaction and stores the accumulated diff.
    pub fn finish_transaction(&mut self) {
        self.tx_diffs.push(core::mem::take(&mut self.current_tx));
    }

    /// Returns collected diffs, consuming the inspector.
    pub fn into_diffs(self) -> Vec<BTreeMap<Address, BTreeMap<StorageKey, StorageValue>>> {
        self.tx_diffs
    }

    fn record_sstore<INTR: InterpreterTypes>(&mut self, interp: &Interpreter<INTR>) {
        let contract = interp.contract.address;
        let stack = &interp.stack;
        if stack.len() < 2 {
            return;
        }
        let slot = stack.peek(0).copied().unwrap_or(U256::ZERO);
        let value = stack.peek(1).copied().unwrap_or(U256::ZERO);
        let slot: StorageKey = B256::from(slot);
        let value: StorageValue = value;
        self.current_tx.entry(contract).or_default().insert(slot, value);
    }
}

impl<CTX, INTR> Inspector<CTX, INTR> for StorageDiffInspector
where
    CTX: ContextTr,
    INTR: InterpreterTypes,
{
    fn step(&mut self, interp: &mut Interpreter<INTR>, _context: &mut CTX) {
        if let Some(opcode) = OpCode::new(interp.bytecode.opcode()) {
            if opcode == OpCode::SSTORE {
                self.record_sstore(interp);
            }
        }
    }
}
