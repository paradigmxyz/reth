//! Geth trace builder

use crate::tracing::{
    types::{CallTraceNode, CallTraceStepStackItem},
    TracingInspectorConfig,
};
use reth_primitives::{Address, Bytes, H256, U256};
use reth_rpc_types::trace::geth::{
    AccountState, CallConfig, CallFrame, DefaultFrame, DiffMode, GethDefaultTracingOptions,
    PreStateConfig, PreStateFrame, PreStateMode, StructLog,
};
use revm::{db::DatabaseRef, primitives::ResultAndState};
use std::collections::{BTreeMap, HashMap, VecDeque};

/// A type for creating geth style traces
#[derive(Clone, Debug)]
pub struct GethTraceBuilder {
    /// Recorded trace nodes.
    nodes: Vec<CallTraceNode>,
    /// How the traces were recorded
    _config: TracingInspectorConfig,
}

impl GethTraceBuilder {
    /// Returns a new instance of the builder
    pub(crate) fn new(nodes: Vec<CallTraceNode>, _config: TracingInspectorConfig) -> Self {
        Self { nodes, _config }
    }

    /// Fill in the geth trace with all steps of the trace and its children traces in the order they
    /// appear in the transaction.
    fn fill_geth_trace(
        &self,
        main_trace_node: &CallTraceNode,
        opts: &GethDefaultTracingOptions,
        storage: &mut HashMap<Address, BTreeMap<H256, H256>>,
        struct_logs: &mut Vec<StructLog>,
    ) {
        // A stack with all the steps of the trace and all its children's steps.
        // This is used to process the steps in the order they appear in the transactions.
        // Steps are grouped by their Call Trace Node, in order to process them all in the order
        // they appear in the transaction, we need to process steps of call nodes when they appear.
        // When we find a call step, we push all the steps of the child trace on the stack, so they
        // are processed next. The very next step is the last item on the stack
        let mut step_stack = VecDeque::with_capacity(main_trace_node.trace.steps.len());

        main_trace_node.push_steps_on_stack(&mut step_stack);

        // Iterate over the steps inside the given trace
        while let Some(CallTraceStepStackItem { trace_node, step, call_child_id }) =
            step_stack.pop_back()
        {
            let mut log = step.convert_to_geth_struct_log(opts);

            // Fill in memory and storage depending on the options
            if opts.is_storage_enabled() {
                let contract_storage = storage.entry(step.contract).or_default();
                if let Some(change) = step.storage_change {
                    contract_storage.insert(change.key.into(), change.value.into());
                    log.storage = Some(contract_storage.clone());
                }
            }

            if opts.is_return_data_enabled() {
                log.return_data = Some(trace_node.trace.output.clone().into());
            }

            // Add step to geth trace
            struct_logs.push(log);

            // If the step is a call, we first push all the steps of the child trace on the stack,
            // so they are processed next
            if let Some(call_child_id) = call_child_id {
                let child_trace = &self.nodes[call_child_id];
                child_trace.push_steps_on_stack(&mut step_stack);
            }
        }
    }

    /// Generate a geth-style trace e.g. for `debug_traceTransaction`
    ///
    /// This expects the gas used and return value for the
    /// [ExecutionResult](revm::primitives::ExecutionResult) of the executed transaction.
    pub fn geth_traces(
        &self,
        receipt_gas_used: u64,
        return_value: Bytes,
        opts: GethDefaultTracingOptions,
    ) -> DefaultFrame {
        if self.nodes.is_empty() {
            return Default::default()
        }
        // Fetch top-level trace
        let main_trace_node = &self.nodes[0];
        let main_trace = &main_trace_node.trace;

        let mut struct_logs = Vec::new();
        let mut storage = HashMap::new();
        self.fill_geth_trace(main_trace_node, &opts, &mut storage, &mut struct_logs);

        DefaultFrame {
            // If the top-level trace succeeded, then it was a success
            failed: !main_trace.success,
            gas: receipt_gas_used,
            return_value,
            struct_logs,
        }
    }

    /// Generate a geth-style traces for the call tracer.
    ///
    /// This decodes all call frames from the recorded traces.
    ///
    /// This expects the gas used and return value for the
    /// [ExecutionResult](revm::primitives::ExecutionResult) of the executed transaction.
    pub fn geth_call_traces(&self, opts: CallConfig, gas_used: u64) -> CallFrame {
        if self.nodes.is_empty() {
            return Default::default()
        }

        let include_logs = opts.with_log.unwrap_or_default();
        // first fill up the root
        let main_trace_node = &self.nodes[0];
        let mut root_call_frame = main_trace_node.geth_empty_call_frame(include_logs);
        root_call_frame.gas_used = U256::from(gas_used);

        // selfdestructs are not recorded as individual call traces but are derived from
        // the call trace and are added as additional `CallFrame` objects to the parent call
        if let Some(selfdestruct) = main_trace_node.geth_selfdestruct_call_trace() {
            root_call_frame.calls.push(selfdestruct);
        }

        if opts.only_top_call.unwrap_or_default() {
            return root_call_frame
        }

        // fill all the call frames in the root call frame with the recorded traces.
        // traces are identified by their index in the arena
        // so we can populate the call frame tree by walking up the call tree
        let mut call_frames = Vec::with_capacity(self.nodes.len());
        call_frames.push((0, root_call_frame));

        for (idx, trace) in self.nodes.iter().enumerate().skip(1) {
            // selfdestructs are not recorded as individual call traces but are derived from
            // the call trace and are added as additional `CallFrame` objects to the parent call
            if let Some(selfdestruct) = trace.geth_selfdestruct_call_trace() {
                call_frames.last_mut().expect("not empty").1.calls.push(selfdestruct);
            }
            call_frames.push((idx, trace.geth_empty_call_frame(include_logs)));
        }

        // pop the _children_ calls frame and move it to the parent
        // this will roll up the child frames to their parent; this works because `child idx >
        // parent idx`
        loop {
            let (idx, call) = call_frames.pop().expect("call frames not empty");
            let node = &self.nodes[idx];
            if let Some(parent) = node.parent {
                let parent_frame = &mut call_frames[parent];
                // we need to ensure that calls are in order they are called: the last child node is
                // the last call, but since we walk up the tree, we need to always
                // insert at position 0
                parent_frame.1.calls.insert(0, call);
            } else {
                debug_assert!(call_frames.is_empty(), "only one root node has no parent");
                return call
            }
        }
    }

    ///  Returns the accounts necessary for transaction execution.
    ///
    /// The prestate mode returns the accounts necessary to execute a given transaction.
    /// diff_mode returns the differences between the transaction's pre and post-state.
    ///
    /// * `state` - The state post-transaction execution.
    /// * `diff_mode` - if prestate is in diff or prestate mode.
    /// * `db` - The database to fetch state pre-transaction execution.
    pub fn geth_prestate_traces<DB>(
        &self,
        ResultAndState { state, .. }: &ResultAndState,
        prestate_config: PreStateConfig,
        db: DB,
    ) -> Result<PreStateFrame, DB::Error>
    where
        DB: DatabaseRef,
    {
        let account_diffs: Vec<_> =
            state.into_iter().map(|(addr, acc)| (*addr, &acc.info)).collect();

        if !prestate_config.is_diff_mode() {
            let mut prestate = PreStateMode::default();
            for (addr, _) in account_diffs {
                let db_acc = db.basic(addr)?.unwrap_or_default();
                prestate.0.insert(
                    addr,
                    AccountState {
                        balance: Some(db_acc.balance),
                        nonce: Some(U256::from(db_acc.nonce)),
                        code: db_acc.code.as_ref().map(|code| Bytes::from(code.original_bytes())),
                        storage: None,
                    },
                );
            }
            self.update_storage_from_trace(&mut prestate.0, false);
            Ok(PreStateFrame::Default(prestate))
        } else {
            let mut state_diff = DiffMode::default();
            for (addr, changed_acc) in account_diffs {
                let db_acc = db.basic(addr)?.unwrap_or_default();
                let pre_state = AccountState {
                    balance: Some(db_acc.balance),
                    nonce: Some(U256::from(db_acc.nonce)),
                    code: db_acc.code.as_ref().map(|code| Bytes::from(code.original_bytes())),
                    storage: None,
                };
                let post_state = AccountState {
                    balance: Some(changed_acc.balance),
                    nonce: Some(U256::from(changed_acc.nonce)),
                    code: changed_acc.code.as_ref().map(|code| Bytes::from(code.original_bytes())),
                    storage: None,
                };
                state_diff.pre.insert(addr, pre_state);
                state_diff.post.insert(addr, post_state);
            }
            self.update_storage_from_trace(&mut state_diff.pre, false);
            self.update_storage_from_trace(&mut state_diff.post, true);
            Ok(PreStateFrame::Diff(state_diff))
        }
    }

    fn update_storage_from_trace(
        &self,
        account_states: &mut BTreeMap<Address, AccountState>,
        post_value: bool,
    ) {
        for node in self.nodes.iter() {
            node.geth_update_account_storage(account_states, post_value);
        }
    }
}
