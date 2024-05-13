//! Ephemeral context for custom instructions.
//!
//! Management of state available for custom instructions during the execution
//! of a single transaction.

use std::{cell::RefCell, collections::HashMap, rc::Rc};

#[derive(Clone, Default)]
/// Context variables to be used in instructions. The data set here is expected
/// to live for the duration of a single transaction.
/// Similar to TStore for arbitrary data.
pub struct InstructionsContext {
    /// Contains the actual variables. Is meant to be accessed both for reads
    /// and writes using interior mutability, so that the Instruction and
    /// BoxedInstruction signatures are observed.
    inner: Rc<RefCell<HashMap<&'static str, Vec<u8>>>>,
}

impl InstructionsContext {
    /// Sets a value for the given key.
    pub fn set(&self, key: &'static str, value: Vec<u8>) {
        let _ = self.inner.borrow_mut().insert(key, value);
    }

    /// Gets the value for the given key, if any.
    pub fn get(&self, key: &'static str) -> Option<Vec<u8>> {
        self.inner.borrow().get(&key).cloned()
    }

    /// Removes the value for the given key.
    pub fn remove(&self, key: &'static str) -> Option<Vec<u8>> {
        self.inner.borrow_mut().remove(&key)
    }

    /// Empties inner state.
    pub fn clear(&self) {
        self.inner.borrow_mut().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_revm::{Context, Evm, InMemoryDB};
    use revm_interpreter::Interpreter;
    use revm_primitives::{address, AccountInfo, Bytecode, TransactTo, U256};
    use std::sync::Arc;

    #[test]
    fn test_set_get() {
        let ctx = InstructionsContext::default();
        let key = "my-key";
        let value = vec![0x01, 0x02];

        ctx.set(key, value.clone());
        assert_eq!(ctx.get(key).unwrap(), value);
    }

    #[test]
    fn test_context_variables_are_available_during_tx() {
        let code = Bytecode::new_raw([0xEE, 0xEF, 0x00].into());
        let code_hash = code.hash_slow();
        let to_addr = address!("ffffffffffffffffffffffffffffffffffffffff");

        // initialize the custom context and make sure it's None for a given key
        let custom_context = InstructionsContext::default();
        let key = "my-key";
        assert_eq!(custom_context.get(key), None);

        let to_capture_instructions = custom_context.clone();
        let to_capture_post_execution = custom_context.clone();
        let mut evm = Evm::builder()
            .with_db(InMemoryDB::default())
            .modify_db(|db| {
                db.insert_account_info(to_addr, AccountInfo::new(U256::ZERO, 0, code_hash, code))
            })
            .modify_tx_env(|tx| tx.transact_to = TransactTo::Call(to_addr))
            .append_handler_register_box(Box::new(move |handler| {
                let writer_context = to_capture_instructions.clone();
                let writer_instruction =
                    Box::new(move |_interp: &mut Interpreter, _host: &mut Context<(), _>| {
                        // write into the context variable.
                        writer_context.set(key, vec![0x01, 0x02]);
                    });
                let reader_context = to_capture_instructions.clone();
                let reader_instruction =
                    Box::new(move |_interp: &mut Interpreter, _host: &mut Context<(), _>| {
                        // read from context variable.
                        assert_eq!(reader_context.get(key).unwrap(), vec![0x01, 0x02]);
                    });

                let mut table = handler.take_instruction_table();
                table.insert_boxed(0xEE, writer_instruction);
                table.insert_boxed(0xEF, reader_instruction);
                handler.instruction_table = table;

                let post_execution_context = to_capture_post_execution.clone();
                #[allow(clippy::arc_with_non_send_sync)]
                {
                    handler.post_execution.end = Arc::new(move |_, outcome: _| {
                        post_execution_context.clear();
                        outcome
                    });
                }
            }))
            .build();

        let _result_and_state = evm.transact().unwrap();

        // ensure the custom context was cleared
        assert_eq!(custom_context.get(key), None);
    }
}
