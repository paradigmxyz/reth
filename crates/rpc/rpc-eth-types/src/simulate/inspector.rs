//! Collects simulation logs without inserting synthetic transfers into the EVM journal.

use alloy_primitives::{Address, Log, LogData, U256};
use revm::{
    context_interface::ContextTr,
    interpreter::{CallInputs, CallOutcome, CreateInputs, CreateOutcome},
    Inspector,
};
use revm_inspectors::transfer::{TransferInspector, TRANSFER_EVENT_TOPIC, TRANSFER_LOG_EMITTER};

/// Collects contract logs and synthetic ETH transfer logs in execution order.
///
/// Failed call frames discard their logs. The collection resets at the start of each transaction
/// and never writes synthetic logs to the journal, so tracing cannot change simulated receipts.
#[derive(Debug, Default)]
pub struct SimulateInspector {
    transfers: TransferInspector,
    checkpoints: Vec<usize>,
    /// Collected logs, including synthetic transfers. May be taken between transactions.
    pub logs: Vec<Log>,
}

impl SimulateInspector {
    fn enter_frame(&mut self) {
        if self.checkpoints.is_empty() {
            self.logs.clear();
            self.transfers = TransferInspector::new(false);
        }
        self.checkpoints.push(self.logs.len());
    }

    fn exit_frame(&mut self, success: bool) {
        if let Some(checkpoint) = self.checkpoints.pop() &&
            !success
        {
            self.logs.truncate(checkpoint);
        }
    }
}

impl<CTX: ContextTr> Inspector<CTX> for SimulateInspector {
    fn call(&mut self, context: &mut CTX, inputs: &mut CallInputs) -> Option<CallOutcome> {
        self.enter_frame();
        // CALLCODE checks the balance without transferring value to another account.
        if !inputs.scheme.is_call() {
            return None
        }
        let next_transfer = self.transfers.transfers().len();
        let outcome = self.transfers.call(context, inputs);
        self.logs.extend(
            self.transfers.transfers()[next_transfer..]
                .iter()
                .map(|transfer| transfer_log(transfer.from, transfer.to, transfer.value)),
        );
        outcome
    }

    fn call_end(&mut self, _context: &mut CTX, _inputs: &CallInputs, outcome: &mut CallOutcome) {
        self.exit_frame(outcome.instruction_result().is_ok());
    }

    fn create(&mut self, context: &mut CTX, inputs: &mut CreateInputs) -> Option<CreateOutcome> {
        self.enter_frame();
        let next_transfer = self.transfers.transfers().len();
        let outcome = self.transfers.create(context, inputs);
        self.logs.extend(
            self.transfers.transfers()[next_transfer..]
                .iter()
                .map(|transfer| transfer_log(transfer.from, transfer.to, transfer.value)),
        );
        outcome
    }

    fn create_end(
        &mut self,
        _context: &mut CTX,
        _inputs: &CreateInputs,
        outcome: &mut CreateOutcome,
    ) {
        self.exit_frame(outcome.instruction_result().is_ok());
    }

    fn log(&mut self, _context: &mut CTX, log: Log) {
        self.logs.push(log);
    }

    fn selfdestruct(&mut self, contract: Address, target: Address, value: U256) {
        if !value.is_zero() {
            self.logs.push(transfer_log(contract, target, value));
        }
    }
}

fn transfer_log(from: Address, to: Address, value: U256) -> Log {
    Log {
        address: TRANSFER_LOG_EMITTER,
        data: LogData::new_unchecked(
            vec![TRANSFER_EVENT_TOPIC, from.into_word(), to.into_word()],
            value.to_be_bytes::<32>().into(),
        ),
    }
}
