//! Ordering adapter for speculative BAL worker results.

use super::{
    worker::{BalWorkerError, BalWorkerOutput},
    BalExecutionError,
};
use crossbeam_channel::Receiver;

#[derive(Debug, thiserror::Error)]
pub(super) enum OrderedWorkerOutputError {
    #[error(transparent)]
    Worker(#[from] BalWorkerError),
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

pub(super) fn ordered_worker_outputs<R>(
    result_rx: &Receiver<Result<BalWorkerOutput<R>, BalWorkerError>>,
    total: usize,
) -> impl Iterator<Item = Result<BalWorkerOutput<R>, OrderedWorkerOutputError>> + '_ {
    OrderedWorkerOutputs::new(result_rx, total)
}

struct OrderedWorkerOutputs<'a, R> {
    result_rx: &'a Receiver<Result<BalWorkerOutput<R>, BalWorkerError>>,
    pending: Vec<Option<BalWorkerOutput<R>>>,
    next: usize,
    total: usize,
    failed: bool,
}

impl<'a, R> OrderedWorkerOutputs<'a, R> {
    fn new(
        result_rx: &'a Receiver<Result<BalWorkerOutput<R>, BalWorkerError>>,
        total: usize,
    ) -> Self {
        Self {
            result_rx,
            pending: (0..total).map(|_| None).collect(),
            next: 0,
            total,
            failed: false,
        }
    }
}

impl<R> Iterator for OrderedWorkerOutputs<'_, R> {
    type Item = Result<BalWorkerOutput<R>, OrderedWorkerOutputError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.next >= self.total {
            return None
        }

        loop {
            if let Some(output) = self.pending[self.next].take() {
                self.next += 1;
                return Some(Ok(output))
            }

            let output = match self.result_rx.recv() {
                Ok(Ok(output)) => output,
                Ok(Err(err)) => {
                    self.failed = true;
                    return Some(Err(err.into()))
                }
                Err(_) => {
                    self.failed = true;
                    return Some(Err(OrderedWorkerOutputError::ResultChannelClosed))
                }
            };

            let index = output.index;
            assert!(
                index < self.total,
                "BAL worker returned out-of-bounds transaction index {index}; total={}",
                self.total
            );
            assert!(
                index >= self.next && self.pending[index].is_none(),
                "BAL worker returned duplicate transaction index {index}",
            );
            self.pending[index] = Some(output);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Address;
    use crossbeam_channel::unbounded;

    fn output(index: usize) -> BalWorkerOutput<usize> {
        BalWorkerOutput { index, signer: Address::ZERO, result: index }
    }

    #[test]
    fn yields_worker_outputs_in_transaction_order() {
        let (tx, rx) = unbounded();
        tx.send(Ok(output(2))).unwrap();
        tx.send(Ok(output(0))).unwrap();
        tx.send(Ok(output(1))).unwrap();

        let indexes = ordered_worker_outputs(&rx, 3)
            .map(|result| result.expect("ordered output").index)
            .collect::<Vec<_>>();

        assert_eq!(indexes, [0, 1, 2]);
    }

    #[test]
    fn reports_a_closed_result_channel() {
        let (tx, rx) = unbounded::<Result<BalWorkerOutput<usize>, BalWorkerError>>();
        drop(tx);

        assert!(matches!(
            ordered_worker_outputs(&rx, 1).next(),
            Some(Err(OrderedWorkerOutputError::ResultChannelClosed))
        ));
    }

    #[test]
    #[should_panic(expected = "out-of-bounds transaction index")]
    fn rejects_out_of_bounds_worker_indexes() {
        let (tx, rx) = unbounded();
        tx.send(Ok(output(1))).unwrap();

        let _ = ordered_worker_outputs(&rx, 1).next();
    }

    #[test]
    #[should_panic(expected = "duplicate transaction index")]
    fn rejects_duplicate_worker_indexes() {
        let (tx, rx) = unbounded();
        tx.send(Ok(output(1))).unwrap();
        tx.send(Ok(output(1))).unwrap();

        let _ = ordered_worker_outputs(&rx, 2).next();
    }
}
