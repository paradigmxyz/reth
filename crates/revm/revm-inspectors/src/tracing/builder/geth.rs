//! Geth trace builder

use crate::tracing::{
    types::{CallTraceNode, CallTraceStepStackItem},
    utils::load_account_code,
    TracingInspectorConfig,
};
use alloy_primitives::{Address, Bytes, B256, U256};
use reth_rpc_types::trace::geth::{
    AccountChangeKind, AccountState, CallConfig, CallFrame, DefaultFrame, DiffMode,
    GethDefaultTracingOptions, PreStateConfig, PreStateFrame, PreStateMode, StructLog,
};
use revm::{db::DatabaseRef, primitives::ResultAndState};
use std::collections::{btree_map::Entry, BTreeMap, HashMap, VecDeque};

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
    pub fn new(nodes: Vec<CallTraceNode>, _config: TracingInspectorConfig) -> Self {
        Self { nodes, _config }
    }

    /// Fill in the geth trace with all steps of the trace and its children traces in the order they
    /// appear in the transaction.
    fn fill_geth_trace(
        &self,
        main_trace_node: &CallTraceNode,
        opts: &GethDefaultTracingOptions,
        storage: &mut HashMap<Address, BTreeMap<B256, B256>>,
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
                log.return_data = Some(trace_node.trace.output.clone());
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
    pub fn geth_prestate_traces<DB: DatabaseRef>(
        &self,
        ResultAndState { state, .. }: &ResultAndState,
        prestate_config: PreStateConfig,
        db: DB,
    ) -> Result<PreStateFrame, DB::Error> {
        let account_diffs = state.into_iter().map(|(addr, acc)| (*addr, acc));

        if prestate_config.is_default_mode() {
            let mut prestate = PreStateMode::default();
            // in default mode we __only__ return the touched state
            for node in self.nodes.iter() {
                let addr = node.trace.address;

                let acc_state = match prestate.0.entry(addr) {
                    Entry::Vacant(entry) => {
                        let db_acc = db.basic_ref(addr)?.unwrap_or_default();
                        let code = load_account_code(&db, &db_acc);
                        let acc_state =
                            AccountState::from_account_info(db_acc.nonce, db_acc.balance, code);
                        entry.insert(acc_state)
                    }
                    Entry::Occupied(entry) => entry.into_mut(),
                };

                for (key, value) in node.touched_slots() {
                    match acc_state.storage.entry(key.into()) {
                        Entry::Vacant(entry) => {
                            entry.insert(value.into());
                        }
                        Entry::Occupied(_) => {
                            // we've already recorded this slot
                        }
                    }
                }
            }

            // also need to check changed accounts for things like balance changes etc
            for (addr, changed_acc) in account_diffs {
                let acc_state = match prestate.0.entry(addr) {
                    Entry::Vacant(entry) => {
                        let db_acc = db.basic_ref(addr)?.unwrap_or_default();
                        let code = load_account_code(&db, &db_acc);
                        let acc_state =
                            AccountState::from_account_info(db_acc.nonce, db_acc.balance, code);
                        entry.insert(acc_state)
                    }
                    Entry::Occupied(entry) => {
                        // already recorded via touched accounts
                        entry.into_mut()
                    }
                };

                // in case we missed anything during the trace, we need to add the changed accounts
                // storage
                for (key, slot) in changed_acc.storage.iter() {
                    match acc_state.storage.entry((*key).into()) {
                        Entry::Vacant(entry) => {
                            entry.insert(slot.previous_or_original_value.into());
                        }
                        Entry::Occupied(_) => {
                            // we've already recorded this slot
                        }
                    }
                }
            }

            Ok(PreStateFrame::Default(prestate))
        } else {
            let mut state_diff = DiffMode::default();
            let mut account_change_kinds = HashMap::with_capacity(account_diffs.len());
            for (addr, changed_acc) in account_diffs {
                let db_acc = db.basic_ref(addr)?.unwrap_or_default();

                let pre_code = load_account_code(&db, &db_acc);

                let mut pre_state =
                    AccountState::from_account_info(db_acc.nonce, db_acc.balance, pre_code);

                let mut post_state = AccountState::from_account_info(
                    changed_acc.info.nonce,
                    changed_acc.info.balance,
                    changed_acc.info.code.as_ref().map(|code| code.original_bytes()),
                );

                // handle storage changes
                for (key, slot) in changed_acc.storage.iter().filter(|(_, slot)| slot.is_changed())
                {
                    pre_state.storage.insert((*key).into(), slot.previous_or_original_value.into());
                    post_state.storage.insert((*key).into(), slot.present_value.into());
                }

                state_diff.pre.insert(addr, pre_state);
                state_diff.post.insert(addr, post_state);

                // determine the change type
                let pre_change = if changed_acc.is_created() {
                    AccountChangeKind::Create
                } else {
                    AccountChangeKind::Modify
                };
                let post_change = if changed_acc.is_selfdestructed() {
                    AccountChangeKind::SelfDestruct
                } else {
                    AccountChangeKind::Modify
                };

                account_change_kinds.insert(addr, (pre_change, post_change));
            }

            // ensure we're only keeping changed entries
            state_diff.retain_changed().remove_zero_storage_values();

            self.diff_traces(&mut state_diff.pre, &mut state_diff.post, account_change_kinds);
            Ok(PreStateFrame::Diff(state_diff))
        }
    }

    /// Returns the difference between the pre and post state of the transaction depending on the
    /// kind of changes of that account (pre,post)
    fn diff_traces(
        &self,
        pre: &mut BTreeMap<Address, AccountState>,
        post: &mut BTreeMap<Address, AccountState>,
        change_type: HashMap<Address, (AccountChangeKind, AccountChangeKind)>,
    ) {
        post.retain(|addr, post_state| {
            // Don't keep destroyed accounts in the post state
            if change_type.get(addr).map(|ty| ty.1.is_selfdestruct()).unwrap_or(false) {
                return false
            }
            if let Some(pre_state) = pre.get(addr) {
                // remove any unchanged account info
                post_state.remove_matching_account_info(pre_state);
            }

            true
        });

        // Don't keep created accounts the pre state
        pre.retain(|addr, _pre_state| {
            // only keep accounts that are not created
            change_type.get(addr).map(|ty| !ty.0.is_created()).unwrap_or(true)
        });
    }
}
