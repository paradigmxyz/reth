use alloy_primitives::{Address, StorageKey, StorageValue, B256};
use revm::{
    bytecode::OpCode,
    context::ContextTr,
    inspector::Inspector,
    interpreter::{
        interpreter_types::{InputsTr, Jumps, StackTr},
        Interpreter, InterpreterTypes,
    },
};
use std::collections::{BTreeMap, HashMap};

/// Storage diff per transaction. Outer `Vec` is ordered by transaction index.
pub type StorageDiffs = Vec<HashMap<Address, BTreeMap<StorageKey, StorageValue>>>;

/// Captures per-transaction storage diffs by watching `SSTORE` instructions during replay.
#[derive(Clone, Debug, Default)]
pub struct StorageDiffInspector {
    current_tx: HashMap<Address, BTreeMap<StorageKey, StorageValue>>,
    tx_diffs: StorageDiffs,
}

impl StorageDiffInspector {
    /// Creates a new inspector instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Marks the end of the current transaction and stores the accumulated diff.
    pub fn finish_transaction(&mut self) {
        self.tx_diffs.push(self.current_tx.drain().collect());
    }

    /// Returns collected diffs, consuming the inspector.
    pub fn into_diffs(mut self) -> StorageDiffs {
        if !self.current_tx.is_empty() {
            self.finish_transaction();
        }
        self.tx_diffs
    }

    fn record_sstore<INTR: InterpreterTypes>(&mut self, interp: &Interpreter<INTR>) {
        let contract = interp.input.target_address();
        let stack = &interp.stack;
        let len = stack.len();
        if len < 2 {
            return;
        }

        let data = stack.data();
        let slot: StorageKey = B256::from(data[len - 1]);
        let value: StorageValue = data[len - 2];
        self.current_tx.entry(contract).or_default().insert(slot, value);
    }
}

impl<CTX, INTR> Inspector<CTX, INTR> for StorageDiffInspector
where
    CTX: ContextTr,
    INTR: InterpreterTypes,
{
    fn step(&mut self, interp: &mut Interpreter<INTR>, _context: &mut CTX) {
        if let Some(opcode) = OpCode::new(interp.bytecode.opcode()) &&
            opcode == OpCode::SSTORE
        {
            self.record_sstore(interp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;

    #[test]
    fn finish_transaction_moves_current_diff() {
        let mut inspector = StorageDiffInspector::new();
        let addr = Address::repeat_byte(0x11);
        let key = StorageKey::from(B256::from(U256::from(1)));
        let value: StorageValue = U256::from(2);
        inspector.current_tx.entry(addr).or_default().insert(key, value);

        inspector.finish_transaction();
        assert!(inspector.current_tx.is_empty());
        assert_eq!(inspector.tx_diffs.len(), 1);
        let slots = inspector.tx_diffs[0].get(&addr).unwrap();
        assert_eq!(slots.get(&key), Some(&value));
    }

    #[test]
    fn into_diffs_flushes_pending_tx() {
        let mut inspector = StorageDiffInspector::new();
        let addr = Address::repeat_byte(0x22);
        let key = StorageKey::from(B256::from(U256::from(3)));
        let value: StorageValue = U256::from(4);
        inspector.current_tx.entry(addr).or_default().insert(key, value);

        let diffs = inspector.into_diffs();
        assert_eq!(diffs.len(), 1);
        let slots = diffs[0].get(&addr).unwrap();
        assert_eq!(slots.get(&key), Some(&value));
    }
}
