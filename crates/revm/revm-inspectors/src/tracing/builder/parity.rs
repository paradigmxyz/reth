use super::walker::CallTraceNodeWalkerBF;
use crate::tracing::{
    types::{CallTraceNode, CallTraceStep},
    TracingInspectorConfig,
};
use reth_primitives::{Address, U64};
use reth_rpc_types::{trace::parity::*, TransactionInfo};
use revm::{
    db::DatabaseRef,
    interpreter::opcode,
    primitives::{AccountInfo, ExecutionResult, ResultAndState, KECCAK_EMPTY},
};
use std::collections::{HashSet, VecDeque};

/// A type for creating parity style traces
///
/// Note: Parity style traces always ignore calls to precompiles.
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

    /// Returns the trace addresses of all call nodes in the set
    ///
    /// Each entry in the returned vector represents the [Self::trace_address] of the corresponding
    /// node in the nodes set.
    ///
    /// CAUTION: This also includes precompiles, which have an empty trace address.
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
    ///
    /// Note: if the call node of `idx` is a precompile, the returned trace address will be empty.
    fn trace_address(&self, idx: usize) -> Vec<usize> {
        if idx == 0 {
            // root call has empty traceAddress
            return vec![]
        }
        let mut graph = vec![];
        let mut node = &self.nodes[idx];
        if node.is_precompile() {
            return graph
        }
        while let Some(parent) = node.parent {
            // the index of the child call in the arena
            let child_idx = node.idx;
            node = &self.nodes[parent];
            // find the index of the child call in the parent node
            let call_idx = node
                .children
                .iter()
                .position(|child| *child == child_idx)
                .expect("non precompile child call exists in parent");
            graph.push(call_idx);
        }
        graph.reverse();
        graph
    }

    /// Returns an iterator over all nodes to trace
    ///
    /// This excludes nodes that represent calls to precompiles.
    fn iter_traceable_nodes(&self) -> impl Iterator<Item = &CallTraceNode> {
        self.nodes.iter().filter(|node| !node.is_precompile())
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

        let breadth_first_addresses = if trace_types.contains(&TraceType::VmTrace) {
            CallTraceNodeWalkerBF::new(&self.nodes)
                .map(|node| node.trace.address)
                .collect::<Vec<_>>()
        } else {
            vec![]
        };

        let mut trace_res = self.into_trace_results(result, trace_types);

        // check the state diff case
        if let Some(ref mut state_diff) = trace_res.state_diff {
            populate_account_balance_nonce_diffs(
                state_diff,
                &db,
                state.into_iter().map(|(addr, acc)| (addr, acc.info)),
            )?;
        }

        // check the vm trace case
        if let Some(ref mut vm_trace) = trace_res.vm_trace {
            populate_vm_trace_bytecodes(&db, vm_trace, breadth_first_addresses)?;
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

        let vm_trace =
            if trace_types.contains(&TraceType::VmTrace) { Some(self.vm_trace()) } else { None };

        let mut traces = Vec::with_capacity(if with_traces { self.nodes.len() } else { 0 });
        let mut diff = StateDiff::default();

        for node in self.iter_traceable_nodes() {
            let trace_address = self.trace_address(node.idx);

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

    /// Creates a VM trace by walking over `CallTraceNode`s
    ///
    /// does not have the code fields filled in
    pub fn vm_trace(&self) -> VmTrace {
        match self.nodes.get(0) {
            Some(current) => self.make_vm_trace(current),
            None => VmTrace { code: Default::default(), ops: Vec::new() },
        }
    }

    /// returns a VM trace without the code filled in
    ///
    /// iteratively creaters a VM trace by traversing an arena
    fn make_vm_trace(&self, start: &CallTraceNode) -> VmTrace {
        let mut child_idx_stack: Vec<usize> = Vec::with_capacity(self.nodes.len());
        let mut sub_stack: VecDeque<Option<VmTrace>> = VecDeque::with_capacity(self.nodes.len());

        let mut current = start;
        let mut child_idx: usize = 0;

        // finds the deepest nested calls of each call frame and fills them up bottom to top
        let instructions = loop {
            match current.children.get(child_idx) {
                Some(child) => {
                    child_idx_stack.push(child_idx + 1);

                    child_idx = 0;
                    current = self.nodes.get(*child).expect("there should be a child");
                }
                None => {
                    let mut instructions: Vec<VmInstruction> =
                        Vec::with_capacity(current.trace.steps.len());

                    for step in &current.trace.steps {
                        let maybe_sub = match step.op.u8() {
                            opcode::CALL |
                            opcode::CALLCODE |
                            opcode::DELEGATECALL |
                            opcode::STATICCALL |
                            opcode::CREATE |
                            opcode::CREATE2 => {
                                sub_stack.pop_front().expect("there should be a sub trace")
                            }
                            _ => None,
                        };

                        instructions.push(Self::make_instruction(step, maybe_sub));
                    }

                    match current.parent {
                        Some(parent) => {
                            sub_stack.push_back(Some(VmTrace {
                                code: Default::default(),
                                ops: instructions,
                            }));

                            child_idx = child_idx_stack.pop().expect("there should be a child idx");

                            current = self.nodes.get(parent).expect("there should be a parent");
                        }
                        None => break instructions,
                    }
                }
            }
        };

        VmTrace { code: Default::default(), ops: instructions }
    }

    /// Creates a VM instruction from a [CallTraceStep] and a [VmTrace] for the subcall if there is
    /// one
    fn make_instruction(step: &CallTraceStep, maybe_sub: Option<VmTrace>) -> VmInstruction {
        let maybe_storage = step.storage_change.map(|storage_change| StorageDelta {
            key: storage_change.key,
            val: storage_change.value,
        });

        let maybe_memory = match step.memory.len() {
            0 => None,
            _ => {
                Some(MemoryDelta { off: step.memory_size, data: step.memory.data().clone().into() })
            }
        };

        let maybe_execution = Some(VmExecutedOperation {
            used: step.gas_cost,
            push: step.new_stack.map(|new_stack| new_stack.into()),
            mem: maybe_memory,
            store: maybe_storage,
        });

        VmInstruction {
            pc: step.pc,
            cost: 0, // TODO: use op gas cost
            ex: maybe_execution,
            sub: maybe_sub,
        }
    }
}

/// addresses are presorted via breadth first walk thru [CallTraceNode]s, this  can be done by a
/// walker in [crate::tracing::builder::walker]
///
/// iteratively fill the [VmTrace] code fields
pub(crate) fn populate_vm_trace_bytecodes<DB, I>(
    db: &DB,
    trace: &mut VmTrace,
    breadth_first_addresses: I,
) -> Result<(), DB::Error>
where
    DB: DatabaseRef,
    I: IntoIterator<Item = Address>,
{
    let mut stack: VecDeque<&mut VmTrace> = VecDeque::new();
    stack.push_back(trace);

    let mut addrs = breadth_first_addresses.into_iter();

    while let Some(curr_ref) = stack.pop_front() {
        for op in curr_ref.ops.iter_mut() {
            if let Some(sub) = op.sub.as_mut() {
                stack.push_back(sub);
            }
        }

        let addr = addrs.next().expect("there should be an address");

        let db_acc = db.basic(addr)?.unwrap_or_default();

        let code_hash = if db_acc.code_hash != KECCAK_EMPTY { db_acc.code_hash } else { continue };

        curr_ref.code = db.code_by_hash(code_hash)?.bytecode.into();
    }

    Ok(())
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
