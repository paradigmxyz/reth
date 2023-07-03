use crate::tracing::{types::CallTraceStep, TracingInspectorConfig};
use reth_primitives::Address;
use reth_rpc_types::trace::parity::{
    MemoryDelta, StorageDelta, VmExecutedOperation, VmInstruction, VmTrace,
};
use revm::interpreter::opcode;

use crate::tracing::types::CallTraceNode;
// use reth_rpc_types::trace::parity::VmTrace;

/// type for doing a Depth first walk down the callgraph
#[derive(Debug)]
pub(crate) struct DFWalk;

/// pub crate type for doing a walk down a reth callgraph
#[derive(Debug)]
pub(crate) struct CallTraceNodeWalker<'trace, W> {
    /// the entire arena
    nodes: &'trace Vec<CallTraceNode>,
    curr_idx: usize,

    /// ordered indexes of nodes
    idxs: Vec<usize>,

    phantom: std::marker::PhantomData<W>,
}

#[derive(Debug)]
pub(crate) struct VmTraceWalker<'trace, W> {
    parents: Vec<Option<&'trace VmTrace>>,
    current: &'trace VmTrace,

    idx_stack: Vec<usize>,
    curr_idx: usize,
    started: bool,

    phantom: std::marker::PhantomData<W>,
}

impl<'trace> CallTraceNodeWalker<'trace, DFWalk> {
    pub(crate) fn new(nodes: &'trace Vec<CallTraceNode>) -> Self {
        let mut idxs: Vec<usize> = Vec::with_capacity(nodes.len());

        let mut visited: Vec<bool> = vec![false; nodes.len()];

        // stores the index of the parents children we are on
        // could never have more elements than the number of nodes
        let mut stack: Vec<usize> = Vec::with_capacity(nodes.len());

        // the currnet nodes we are working with
        let mut curr: usize = 0;

        // index in this heights children
        let mut child_idx: usize = 0;
        loop {
            if !visited[curr] {
                visited[curr] = true;
                idxs.push(curr);
            }

            match nodes[curr].children.get(child_idx) {
                Some(next_idx) => {
                    stack.push(child_idx);
                    child_idx = 0;

                    curr = *next_idx;
                }
                None => {
                    match nodes[curr].parent {
                        Some(parent_idx) => {
                            // we are done with this node, so we go back up to the parent
                            curr = parent_idx;

                            child_idx = stack.pop().expect("There should be a value here") + 1;
                        }
                        None => {
                            // we are at the root node, so we are done
                            break;
                        }
                    }
                }
            }
        }

        Self { nodes, curr_idx: 0, idxs, phantom: std::marker::PhantomData }
    }

    /// DFWalked order of input arena
    fn idxs(&self) -> &Vec<usize> {
        &self.idxs
    }

    /// returns the callee address in depth first order
    pub(crate) fn addresses(&self) -> Vec<Address> {
        self.idxs().iter().map(|idx| self.nodes[*idx].trace.address).collect()
    }

    /// returns a VM trace without the code filled in
    pub(crate) fn into_vm_trace(
        &mut self,
        config: &TracingInspectorConfig,
        current: &CallTraceNode,
    ) -> VmTrace {
        let mut instructions: Vec<VmInstruction> = Vec::with_capacity(current.trace.steps.len());

        for step in current.trace.steps.iter() {
            let maybe_sub = match step.op.u8() {
                opcode::CALL
                | opcode::CALLCODE
                | opcode::DELEGATECALL
                | opcode::STATICCALL
                | opcode::CREATE
                | opcode::CREATE2 => {
                    let next = self.next().expect("missing next node");
                    Some(self.into_vm_trace(config, next))
                }
                _ => None,
            };

            instructions.push(Self::make_instruction(step, config, maybe_sub));
        }

        VmTrace { code: Default::default(), ops: instructions }
    }

    /// todo::n config
    ///
    /// Creates a VM instruction from a [CallTraceStep] and a [VmTrace] for the subcall if there is one
    fn make_instruction(
        step: &CallTraceStep,
        _config: &TracingInspectorConfig,
        maybe_sub: Option<VmTrace>,
    ) -> VmInstruction {
        let maybe_storage = match step.storage_change {
            Some(storage_change) => {
                Some(StorageDelta { key: storage_change.key, val: storage_change.value })
            }
            None => None,
        };

        let maybe_memory = match step.memory.len() {
            0 => None,
            _ => {
                Some(MemoryDelta { off: step.memory_size, data: step.memory.data().clone().into() })
            }
        };

        let maybe_execution = Some(VmExecutedOperation {
            used: step.gas_cost,
            push: match step.new_stack {
                Some(new_stack) => Some(new_stack.into()),
                None => None,
            },
            mem: maybe_memory,
            store: maybe_storage,
        });

        VmInstruction {
            pc: step.pc,
            cost: 0, // todo::n
            ex: maybe_execution,
            sub: maybe_sub,
        }
    }
}

impl<'trace> Iterator for CallTraceNodeWalker<'trace, DFWalk> {
    type Item = &'trace CallTraceNode;

    fn next(&mut self) -> Option<Self::Item> {
        if self.curr_idx < self.idxs.len() {
            let node = &self.nodes[self.idxs[self.curr_idx]];
            self.curr_idx += 1;

            Some(node)
        } else {
            None
        }
    }
}

impl<'trace> VmTraceWalker<'trace, DFWalk> {
    pub(crate) fn new(root: &'trace VmTrace) -> Self {
        VmTraceWalker {
            parents: vec![None],
            current: root,
            idx_stack: Vec::new(),
            curr_idx: 0,
            started: false,

            phantom: std::marker::PhantomData,
        }
    }
}

impl<'trace> Iterator for VmTraceWalker<'trace, DFWalk> {
    type Item = &'trace VmTrace;

    fn next(&mut self) -> Option<Self::Item> {
        match self.started {
            true => loop {
                match self.current.ops.get(self.curr_idx) {
                    Some(op) => match &op.sub {
                        Some(sub) => {
                            self.parents.push(Some(self.current));
                            self.current = &sub;

                            self.idx_stack.push(self.curr_idx);
                            self.curr_idx = 0;
                        }
                        // if theres no subcall continue to the next opcode
                        None => {
                            self.curr_idx += 1;
                        }
                    },
                    // there is no more opcodes check for parent
                    None => {
                        match self.parents.pop().expect("There should be a parent here") {
                            Some(parent) => {
                                self.current = parent;

                                self.curr_idx =
                                    self.idx_stack.pop().expect("There should be an index here")
                                        + 1;
                            }
                            // we are back at the root node, so we are done
                            None => break,
                        }
                    }
                }
            },
            false => {
                self.started = true;
            }
        };

        Some(self.current)
    }
}

#[cfg(test)]
mod tests {
    use crate::tracing::builder::walker::CallTraceNodeWalker;
    use crate::tracing::types::CallTraceNode;

    #[test]
    fn test_walker_build() {
        let nodes = vec![
            CallTraceNode { idx: 0, parent: None, children: vec![1, 2], ..Default::default() },
            CallTraceNode { idx: 1, parent: Some(0), children: vec![3], ..Default::default() },
            CallTraceNode { idx: 2, parent: Some(0), children: vec![4], ..Default::default() },
            CallTraceNode { idx: 3, parent: Some(1), children: vec![], ..Default::default() },
            CallTraceNode { idx: 4, parent: Some(2), children: vec![], ..Default::default() },
        ];

        let walker = CallTraceNodeWalker::new(&nodes);
        println!("{:#?}", walker);

        assert_eq!(walker.idxs()[0], 0);
        assert_eq!(walker.idxs()[1], 1);
        assert_eq!(walker.idxs()[2], 3);
        assert_eq!(walker.idxs()[3], 2);
        assert_eq!(walker.idxs()[4], 4);

        assert_eq!(walker.idxs().len(), nodes.len());
    }

    #[test]
    fn test_iter() {
        let nodes = vec![
            CallTraceNode { idx: 0, parent: None, children: vec![1, 2], ..Default::default() },
            CallTraceNode { idx: 1, parent: Some(0), children: vec![3], ..Default::default() },
            CallTraceNode { idx: 2, parent: Some(0), children: vec![4], ..Default::default() },
            CallTraceNode { idx: 3, parent: Some(1), children: vec![], ..Default::default() },
            CallTraceNode { idx: 4, parent: Some(2), children: vec![], ..Default::default() },
        ];

        let walker = CallTraceNodeWalker::new(&nodes);

        let mut i = 0;
        for (idx, node) in walker.enumerate() {
            println!("idx: {},", idx);
            i += 1;
        }

        assert_eq!(i, nodes.len());
    }
}
