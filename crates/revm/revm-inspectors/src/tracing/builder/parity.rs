use crate::tracing::{types::CallTraceNode, TracingInspectorConfig};
use reth_primitives::{Address, U64};
use reth_rpc_types::{trace::parity::*, TransactionInfo};
use revm::{
    db::DatabaseRef,
    primitives::{AccountInfo, ExecutionResult, ResultAndState},
};
use std::collections::HashSet;

/// A type for creating parity style traces
#[derive(Clone, Debug)]
pub struct ParityTraceBuilder {
    /// Recorded trace nodes
    nodes: Vec<CallTraceNode>,
    /// How the traces were recorded
    _config: TracingInspectorConfig,
}

impl ParityTraceBuilder {
    /// Returns a new instance of the builder
    pub(crate) fn new(nodes: Vec<CallTraceNode>, _config: TracingInspectorConfig) -> Self {
        Self { nodes, _config }
    }

    /// Returns a list of all addresses that appeared as callers.
    pub fn callers(&self) -> HashSet<Address> {
        self.nodes.iter().map(|node| node.trace.caller).collect()
    }

    /// Returns the trace addresses of all transactions in the set
    fn trace_addresses(&self) -> Vec<Vec<usize>> {
        let mut all_addresses = Vec::with_capacity(self.nodes.len());
        for idx in 0..self.nodes.len() {
            all_addresses.push(self.trace_address(idx));
        }
        all_addresses
    }

    /// Returns the `traceAddress` of the node in the arena
    ///
    /// The `traceAddress` field of all returned traces, gives the exact location in the call trace
    /// [index in root, index in first CALL, index in second CALL, …].
    ///
    /// # Panics
    ///
    /// if the `idx` does not belong to a node
    fn trace_address(&self, idx: usize) -> Vec<usize> {
        if idx == 0 {
            // root call has empty traceAddress
            return vec![]
        }
        let mut graph = vec![];
        let mut node = &self.nodes[idx];
        while let Some(parent) = node.parent {
            // the index of the child call in the arena
            let child_idx = node.idx;
            node = &self.nodes[parent];
            // find the index of the child call in the parent node
            let call_idx = node
                .children
                .iter()
                .position(|child| *child == child_idx)
                .expect("child exists in parent");
            graph.push(call_idx);
        }
        graph.reverse();
        graph
    }

    /// Returns an iterator over all recorded traces  for `trace_transaction`
    pub fn into_localized_transaction_traces_iter(
        self,
        info: TransactionInfo,
    ) -> impl Iterator<Item = LocalizedTransactionTrace> {
        self.into_transaction_traces_iter().map(move |trace| {
            let TransactionInfo { hash, index, block_hash, block_number, .. } = info;
            LocalizedTransactionTrace {
                trace,
                transaction_position: index,
                transaction_hash: hash,
                block_number,
                block_hash,
            }
        })
    }

    /// Returns an iterator over all recorded traces  for `trace_transaction`
    pub fn into_localized_transaction_traces(
        self,
        info: TransactionInfo,
    ) -> Vec<LocalizedTransactionTrace> {
        self.into_localized_transaction_traces_iter(info).collect()
    }

    /// Consumes the inspector and returns the trace results according to the configured trace
    /// types.
    ///
    /// Warning: If `trace_types` contains [TraceType::StateDiff] the returned [StateDiff] will only
    /// contain accounts with changed state, not including their balance changes because this is not
    /// tracked during inspection and requires the State map returned after inspection. Use
    /// [ParityTraceBuilder::into_trace_results_with_state] to populate the balance and nonce
    /// changes for the [StateDiff] using the [DatabaseRef].
    pub fn into_trace_results(
        self,
        res: ExecutionResult,
        trace_types: &HashSet<TraceType>,
    ) -> TraceResults {
        let output = match res {
            ExecutionResult::Success { output, .. } => output.into_data(),
            ExecutionResult::Revert { output, .. } => output,
            ExecutionResult::Halt { .. } => Default::default(),
        };

        let (trace, vm_trace, state_diff) = self.into_trace_type_traces(trace_types);

        TraceResults { output: output.into(), trace, vm_trace, state_diff }
    }

    /// Consumes the inspector and returns the trace results according to the configured trace
    /// types.
    ///
    /// This also takes the [DatabaseRef] to populate the balance and nonce changes for the
    /// [StateDiff].
    ///
    /// Note: this is considered a convenience method that takes the state map of
    /// [ResultAndState] after inspecting a transaction
    /// with the [TracingInspector](crate::tracing::TracingInspector).
    pub fn into_trace_results_with_state<DB>(
        self,
        res: ResultAndState,
        trace_types: &HashSet<TraceType>,
        db: DB,
    ) -> Result<TraceResults, DB::Error>
    where
        DB: DatabaseRef,
    {
        let ResultAndState { result, state } = res;
        let mut trace_res = self.into_trace_results(result, trace_types);
        if let Some(ref mut state_diff) = trace_res.state_diff {
            populate_account_balance_nonce_diffs(
                state_diff,
                &db,
                state.into_iter().map(|(addr, acc)| (addr, acc.info)),
            )?;
        }
        Ok(trace_res)
    }

    /// Returns the tracing types that are configured in the set
    pub fn into_trace_type_traces(
        self,
        trace_types: &HashSet<TraceType>,
    ) -> (Option<Vec<TransactionTrace>>, Option<VmTrace>, Option<StateDiff>) {
        if trace_types.is_empty() || self.nodes.is_empty() {
            return (None, None, None)
        }

        let with_traces = trace_types.contains(&TraceType::Trace);
        let with_diff = trace_types.contains(&TraceType::StateDiff);

        let vm_trace = if trace_types.contains(&TraceType::VmTrace) {
            Some(vm_trace(&self.nodes))
        } else {
            None
        };

        let trace_addresses = self.trace_addresses();
        let mut traces = Vec::with_capacity(if with_traces { self.nodes.len() } else { 0 });
        let mut diff = StateDiff::default();

        for (node, trace_address) in self.nodes.iter().zip(trace_addresses) {
            // skip precompiles
            if node.is_precompile() {
                continue
            }

            if with_traces {
                let trace = node.parity_transaction_trace(trace_address);
                traces.push(trace);
            }
            if with_diff {
                node.parity_update_state_diff(&mut diff);
            }
        }

        let traces = with_traces.then_some(traces);
        let diff = with_diff.then_some(diff);

        (traces, vm_trace, diff)
    }

    /// Returns an iterator over all recorded traces  for `trace_transaction`
    pub fn into_transaction_traces_iter(self) -> impl Iterator<Item = TransactionTrace> {
        let trace_addresses = self.trace_addresses();
        self.nodes
            .into_iter()
            .zip(trace_addresses)
            .filter(|(node, _)| !node.is_precompile())
            .map(|(node, trace_address)| node.parity_transaction_trace(trace_address))
    }

    /// Returns the raw traces of the transaction
    pub fn into_transaction_traces(self) -> Vec<TransactionTrace> {
        self.into_transaction_traces_iter().collect()
    }
}

/// Construct the vmtrace for the entire callgraph
fn vm_trace(nodes: &[CallTraceNode]) -> VmTrace {
    // TODO: populate vm trace

    VmTrace { code: nodes[0].trace.data.clone().into(), ops: vec![] }
}

/// Loops over all state accounts in the accounts diff that contains all accounts that are included
/// in the [ExecutionResult] state map and compares the balance and nonce against what's in the
/// `db`, which should point to the beginning of the transaction.
///
/// It's expected that `DB` is a [CacheDB](revm::db::CacheDB) which at this point already contains
/// all the accounts that are in the state map and never has to fetch them from disk.
pub fn populate_account_balance_nonce_diffs<DB, I>(
    state_diff: &mut StateDiff,
    db: DB,
    account_diffs: I,
) -> Result<(), DB::Error>
where
    I: IntoIterator<Item = (Address, AccountInfo)>,
    DB: DatabaseRef,
{
    for (addr, changed_acc) in account_diffs.into_iter() {
        let entry = state_diff.entry(addr).or_default();
        let db_acc = db.basic(addr)?.unwrap_or_default();
        entry.balance = if db_acc.balance == changed_acc.balance {
            Delta::Unchanged
        } else {
            Delta::Changed(ChangedType { from: db_acc.balance, to: changed_acc.balance })
        };
        entry.nonce = if db_acc.nonce == changed_acc.nonce {
            Delta::Unchanged
        } else {
            Delta::Changed(ChangedType {
                from: U64::from(db_acc.nonce),
                to: U64::from(changed_acc.nonce),
            })
        };
    }

    Ok(())
}
