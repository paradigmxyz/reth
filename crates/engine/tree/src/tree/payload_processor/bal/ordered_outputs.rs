//! Ordering adapter for speculative BAL worker results.

use super::{
    worker::{BalWorkerError, BalWorkerOutput},
    BalExecutionError,
};
use crossbeam_channel::Receiver;
use std::collections::VecDeque;

#[derive(Debug, thiserror::Error)]
pub(super) enum OrderedWorkerOutputError {
    /// A worker returned an error.
    #[error(transparent)]
    Worker(#[from] BalWorkerError),
    /// The result channel closed before every transaction had an output.
    #[error("BAL worker result channel closed while waiting for ordered outputs")]
    ResultChannelClosed,
}

impl From<OrderedWorkerOutputError> for BalExecutionError {
    fn from(err: OrderedWorkerOutputError) -> Self {
        match err {
            OrderedWorkerOutputError::Worker(err) => err.into(),
            other => Self::other(other),
        }
    }
}

/// Returns a blocking iterator over worker outputs in transaction order.
///
/// Workers may finish transactions out of order. This adapter buffers future-indexed outputs and
/// yields each output only when every earlier transaction has been yielded. It owns no execution
/// state and performs no canonical commit work.
///
/// Behavior:
/// - each yielded `Ok` output has the next transaction index
/// - worker errors are forwarded unchanged
/// - closed channels before `total` outputs yield `Err`
/// - out-of-bounds and duplicate indices panic because they violate the internal worker/dispatcher
///   index invariant
/// - after the first error, the iterator is exhausted
pub(super) fn ordered_worker_outputs<R>(
    result_rx: &Receiver<Result<BalWorkerOutput<R>, BalWorkerError>>,
    total: usize,
) -> impl Iterator<Item = Result<BalWorkerOutput<R>, OrderedWorkerOutputError>> + '_ {
    OrderedWorkerOutputs::new(result_rx, total)
}

struct OrderedWorkerOutputs<'a, R> {
    result_rx: &'a Receiver<Result<BalWorkerOutput<R>, BalWorkerError>>,
    pending: VecDeque<Option<BalWorkerOutput<R>>>,
    pending_base: usize,
    next: usize,
    total: usize,
    failed: bool,
}

impl<'a, R> OrderedWorkerOutputs<'a, R> {
    fn new(
        result_rx: &'a Receiver<Result<BalWorkerOutput<R>, BalWorkerError>>,
        total: usize,
    ) -> Self {
        Self { result_rx, pending: VecDeque::new(), pending_base: 0, next: 0, total, failed: false }
    }

    fn pop_ready(&mut self) -> Option<BalWorkerOutput<R>> {
        if self.pending.is_empty() || self.next < self.pending_base {
            return None;
        }

        let offset = self.next - self.pending_base;
        if offset >= self.pending.len() {
            return None;
        }

        for _ in 0..offset {
            debug_assert!(self.pending.pop_front().is_some_and(|output| output.is_none()));
            self.pending_base += 1;
        }

        let output = self.pending.front_mut()?.take()?;
        self.pending.pop_front();
        self.pending_base += 1;
        Some(output)
    }

    fn push_pending(&mut self, output: BalWorkerOutput<R>) {
        let index = output.index;
        if self.pending.is_empty() {
            self.pending_base = self.next;
        }

        let offset = index - self.pending_base;
        if offset >= self.pending.len() {
            self.pending.resize_with(offset + 1, || None);
        }

        assert!(
            self.pending[offset].is_none(),
            "BAL worker returned duplicate transaction index {index}"
        );
        self.pending[offset] = Some(output);
    }
}

impl<R> Iterator for OrderedWorkerOutputs<'_, R> {
    type Item = Result<BalWorkerOutput<R>, OrderedWorkerOutputError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.next >= self.total {
            return None;
        }

        loop {
            if let Some(output) = self.pop_ready() {
                self.next += 1;
                return Some(Ok(output));
            }

            let output = match self.result_rx.recv() {
                Ok(Ok(output)) => output,
                Ok(Err(err)) => {
                    self.failed = true;
                    return Some(Err(err.into()));
                }
                Err(_) => {
                    self.failed = true;
                    return Some(Err(OrderedWorkerOutputError::ResultChannelClosed));
                }
            };

            let index = output.index;
            assert!(
                index < self.total,
                "BAL worker returned out-of-bounds transaction index {index}; total={}",
                self.total
            );
            assert!(index >= self.next, "BAL worker returned duplicate transaction index {index}",);

            if index == self.next {
                self.next += 1;
                return Some(Ok(output));
            }

            self.push_pending(output);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::payload_processor::bal::BalExecutionError;
    use alloy_primitives::Address;

    fn output(index: usize, result: u64) -> BalWorkerOutput<u64> {
        BalWorkerOutput { index, signer: Address::ZERO, tx_gas_limit: 0, result }
    }

    fn expect_err_contains<R>(
        result: Result<BalWorkerOutput<R>, OrderedWorkerOutputError>,
        text: &str,
    ) {
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
        tx.send(Err(BalWorkerError::Setup(BalExecutionError::Execution(
            alloy_evm::block::BlockExecutionError::msg("worker failed"),
        ))))
        .unwrap();
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
    #[should_panic(expected = "out-of-bounds transaction index")]
    fn panics_on_out_of_bounds_index() {
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(Ok(output(1, 10))).unwrap();
        drop(tx);

        let mut outputs = ordered_worker_outputs(&rx, 1);
        let _ = outputs.next();
    }

    #[test]
    #[should_panic(expected = "duplicate transaction index")]
    fn panics_on_duplicate_pending_index() {
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(Ok(output(1, 10))).unwrap();
        tx.send(Ok(output(1, 11))).unwrap();
        drop(tx);

        let mut outputs = ordered_worker_outputs(&rx, 2);
        let _ = outputs.next();
    }

    #[test]
    #[should_panic(expected = "duplicate transaction index")]
    fn panics_on_duplicate_already_yielded_index() {
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(Ok(output(0, 0))).unwrap();
        tx.send(Ok(output(0, 1))).unwrap();
        drop(tx);

        let mut outputs = ordered_worker_outputs(&rx, 2);

        assert_eq!(outputs.next().expect("first item").expect("first output").result, 0);
        let _ = outputs.next();
    }
}
