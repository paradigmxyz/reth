//! Ordering adapter for speculative BAL worker results.

use super::{worker::BalWorkerOutput, BalExecutionError};
use alloy_evm::block::BlockExecutionError;
use crossbeam_channel::Receiver;

/// Returns a blocking iterator over worker outputs in transaction order.
///
/// Workers may finish transactions out of order. This adapter buffers future-indexed outputs and
/// yields each output only when every earlier transaction has been yielded. It owns no execution
/// state and performs no canonical commit work.
///
/// Contract for callers:
/// - each yielded `Ok` output has the next transaction index
/// - worker errors are forwarded unchanged
/// - closed channels before `total` outputs, out-of-bounds indices, and duplicate indices yield
///   `Err`
/// - after the first error, the iterator is exhausted
///
/// Callers that intentionally abort workers must stop polling this iterator after initiating
/// abort. Before `total` outputs are yielded, channel closure is treated as an execution error.
pub(super) fn ordered_worker_outputs<R>(
    result_rx: &Receiver<Result<BalWorkerOutput<R>, BalExecutionError>>,
    total: usize,
) -> impl Iterator<Item = Result<BalWorkerOutput<R>, BalExecutionError>> + '_ {
    OrderedWorkerOutputs::new(result_rx, total)
}

struct OrderedWorkerOutputs<'a, R> {
    result_rx: &'a Receiver<Result<BalWorkerOutput<R>, BalExecutionError>>,
    pending: Vec<Option<BalWorkerOutput<R>>>,
    next: usize,
    total: usize,
    failed: bool,
}

impl<'a, R> OrderedWorkerOutputs<'a, R> {
    fn new(
        result_rx: &'a Receiver<Result<BalWorkerOutput<R>, BalExecutionError>>,
        total: usize,
    ) -> Self {
        Self { result_rx, pending: Vec::new(), next: 0, total, failed: false }
    }
}

impl<R> Iterator for OrderedWorkerOutputs<'_, R> {
    type Item = Result<BalWorkerOutput<R>, BalExecutionError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.next >= self.total {
            return None;
        }

        loop {
            if self.next < self.pending.len() {
                if let Some(output) = self.pending[self.next].take() {
                    self.next += 1;
                    return Some(Ok(output));
                }
            }

            let output = match self.result_rx.recv() {
                Ok(Ok(output)) => output,
                Ok(Err(err)) => {
                    self.failed = true;
                    return Some(Err(err));
                }
                Err(_) => {
                    self.failed = true;
                    return Some(Err(BalExecutionError::Evm(BlockExecutionError::msg(
                        "BAL worker result channel closed while waiting for ordered outputs",
                    ))));
                }
            };

            let index = output.index;
            if index >= self.total {
                self.failed = true;
                return Some(Err(BalExecutionError::Evm(BlockExecutionError::msg(
                    "BAL worker returned out-of-bounds transaction index",
                ))));
            }

            if index == self.next {
                self.next += 1;
                return Some(Ok(output));
            }

            if index < self.next {
                self.failed = true;
                return Some(Err(BalExecutionError::Evm(BlockExecutionError::msg(
                    "BAL worker returned duplicate transaction index",
                ))));
            }

            if self.pending.len() <= index {
                self.pending.resize_with(index + 1, || None);
            }

            if self.pending[index].is_some() {
                self.failed = true;
                return Some(Err(BalExecutionError::Evm(BlockExecutionError::msg(
                    "BAL worker returned duplicate transaction index",
                ))));
            }

            self.pending[index] = Some(output);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Address;

    fn output(index: usize, result: u64) -> BalWorkerOutput<u64> {
        BalWorkerOutput { index, signer: Address::ZERO, tx_gas_limit: 0, result }
    }

    fn expect_err_contains<R>(result: Result<BalWorkerOutput<R>, BalExecutionError>, text: &str) {
        let Err(err) = result else {
            panic!("expected ordered worker output error");
        };
        assert!(err.to_string().contains(text), "expected `{err}` to contain `{text}`");
    }

    #[test]
    fn yields_outputs_in_transaction_order() {
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(Ok(output(2, 20))).unwrap();
        tx.send(Ok(output(0, 0))).unwrap();
        tx.send(Ok(output(1, 10))).unwrap();
        drop(tx);

        let results = ordered_worker_outputs(&rx, 3)
            .map(|output| output.expect("ordered output").result)
            .collect::<Vec<_>>();

        assert_eq!(results, vec![0, 10, 20]);
    }

    #[test]
    fn forwards_worker_errors_and_then_stops() {
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(Err(BalExecutionError::Evm(BlockExecutionError::msg("worker failed")))).unwrap();
        drop(tx);

        let mut outputs = ordered_worker_outputs::<u64>(&rx, 1);

        expect_err_contains(outputs.next().expect("first item"), "worker failed");
        assert!(outputs.next().is_none());
    }

    #[test]
    fn rejects_closed_channel_before_all_outputs_arrive() {
        let (tx, rx) = crossbeam_channel::unbounded();
        drop(tx);

        let mut outputs = ordered_worker_outputs::<u64>(&rx, 1);

        expect_err_contains(outputs.next().expect("first item"), "waiting for ordered outputs");
        assert!(outputs.next().is_none());
    }

    #[test]
    fn rejects_out_of_bounds_index() {
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(Ok(output(1, 10))).unwrap();
        drop(tx);

        let mut outputs = ordered_worker_outputs(&rx, 1);

        expect_err_contains(outputs.next().expect("first item"), "out-of-bounds");
        assert!(outputs.next().is_none());
    }

    #[test]
    fn rejects_duplicate_pending_index() {
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(Ok(output(1, 10))).unwrap();
        tx.send(Ok(output(1, 11))).unwrap();
        drop(tx);

        let mut outputs = ordered_worker_outputs(&rx, 2);

        expect_err_contains(outputs.next().expect("first item"), "duplicate");
        assert!(outputs.next().is_none());
    }

    #[test]
    fn rejects_duplicate_already_yielded_index() {
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(Ok(output(0, 0))).unwrap();
        tx.send(Ok(output(0, 1))).unwrap();
        drop(tx);

        let mut outputs = ordered_worker_outputs(&rx, 2);

        assert_eq!(outputs.next().expect("first item").expect("first output").result, 0);
        expect_err_contains(outputs.next().expect("second item"), "duplicate");
        assert!(outputs.next().is_none());
    }
}
