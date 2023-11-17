//! [Inspector] for collecting data reads during EVM execution.

use crate::rw_set::RevmAccessSet;
use reth_primitives::{Address, B256, U256};
use revm::{
    interpreter::{opcode, Interpreter},
    Database, EVMData, Inspector,
};

/// An [Inspector] that collects reads of EVM state.
#[derive(Default, Debug)]
pub struct ReadInspector(RevmAccessSet);

impl ReadInspector {
    /// Create new instance from the access set.
    pub fn new(set: RevmAccessSet) -> Self {
        Self(set)
    }

    /// Consume the inspector and return inner access set.
    pub fn into_inner(self) -> RevmAccessSet {
        self.0
    }
}
impl<DB> Inspector<DB> for ReadInspector
where
    DB: Database,
{
    fn step(&mut self, interpreter: &mut Interpreter<'_>, _data: &mut EVMData<'_, DB>) {
        match interpreter.current_opcode() {
            opcode::SELFBALANCE => {
                self.0.account_balance(interpreter.contract.address);
            }
            opcode::BALANCE => {
                if let Ok(slot) = interpreter.stack().peek(0) {
                    self.0.account_balance(Address::from_word(B256::from(slot.to_be_bytes())));
                }
            }
            opcode::CREATE => {
                // The balance is checked by EVM.
                self.0.account_balance(interpreter.contract.address);
                // The contract address is computed based on nonce.
                self.0.account_nonce(interpreter.contract.address);
            }
            opcode::CREATE2 => {
                // The balance is checked by EVM.
                self.0.account_balance(interpreter.contract.address);
            }
            opcode::EXTCODECOPY | opcode::EXTCODEHASH | opcode::EXTCODESIZE => {
                if let Ok(slot) = interpreter.stack().peek(0) {
                    self.0.account_code(Address::from_word(B256::from(slot.to_be_bytes())));
                }
            }
            opcode::CALL | opcode::CALLCODE => {
                if let Ok(slot) = interpreter.stack().peek(1) {
                    self.0.account_info(Address::from_word(B256::from(slot.to_be_bytes())));
                }
                if let Ok(value) = interpreter.stack.peek(2) {
                    if value != U256::ZERO {
                        self.0.account_balance(interpreter.contract.address);
                    }
                }
            }
            opcode::DELEGATECALL | opcode::STATICCALL => {
                if let Ok(slot) = interpreter.stack().peek(1) {
                    self.0.account_info(Address::from_word(B256::from(slot.to_be_bytes())));
                }
            }
            // `SSTORE` also constitutes a read due to dynamic gas based on the current value.
            opcode::SLOAD | opcode::SSTORE => {
                if let Ok(slot) = interpreter.stack().peek(0) {
                    self.0.slot(interpreter.contract.address, B256::from(slot.to_be_bytes()));
                }
            }
            opcode::SELFDESTRUCT => {
                // We need to read the balance and nonce in order to transfer or reset it.
                self.0.account_nonce(interpreter.contract.address);
                self.0.account_balance(interpreter.contract.address);
                if let Ok(slot) = interpreter.stack.peek(0) {
                    self.0.account_info(Address::from_word(B256::from(slot.to_be_bytes())));
                }
            }
            _ => (),
        }
    }
}
