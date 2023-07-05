use std::collections::VecDeque;
use crate::tracing::{types::CallTraceStep, TracingInspectorConfig};
use reth_primitives::Address;
use reth_rpc_types::trace::parity::{
    MemoryDelta, StorageDelta, VmExecutedOperation, VmInstruction, VmTrace,
};
use revm::interpreter::opcode;

use crate::tracing::types::CallTraceNode;
// use reth_rpc_types::trace::parity::VmTrace;

/// lazy eval iterator over an arena depth first
#[derive(Debug, Clone)]
pub(crate) struct CallTraceNodeWalkerDF<'trace> {
    /// the entire arena
    nodes: &'trace Vec<CallTraceNode>,
    curr_idx: usize,

    stack: Vec<usize>,
    visited: Vec<bool>,
}

impl<'trace> CallTraceNodeWalkerDF<'trace> {
    pub(crate) fn new(nodes: &'trace Vec<CallTraceNode>) -> Self {
        Self {
            nodes,
            curr_idx: 0,
            /// can never be deeper than len of arena
            stack: Vec::with_capacity(nodes.len()),
            visited: vec![false; nodes.len()],
        }
    }
}

impl<'trace> Iterator for CallTraceNodeWalkerDF<'trace> {
    type Item = &'trace CallTraceNode;

    fn next(&mut self) -> Option<Self::Item> {
        let mut next_child = 0;
        loop {
            if !self.visited[self.curr_idx] {
                self.visited[self.curr_idx] = true;
                return self.nodes.get(self.curr_idx);
            }

            let curr = self.nodes.get(self.curr_idx).expect("missing node");

            match curr.children.get(next_child) {
                Some(child_idx) => {
                    self.stack.push(next_child + 1);

                    self.curr_idx = *child_idx;
                }
                None => match curr.parent {
                    Some(parent_idx) => {
                        next_child = self.stack.pop().expect("missing stack");
                        self.curr_idx = parent_idx;
                    }
                    None => {
                        return None;
                    }
                },
            }
        }
    }
}

/// lazt eval iterator over an arena breadth first
pub(crate) struct CallTraceNodeWalkerBF<'trace> {
    /// the entire arena
    nodes: &'trace Vec<CallTraceNode>,

    queue: VecDeque<usize>,
}

impl<'trace> CallTraceNodeWalkerBF<'trace> {
    pub(crate) fn new(nodes: &'trace Vec<CallTraceNode>) -> Self {
        let mut queue = VecDeque::with_capacity(nodes.len());
        queue.push_back(0);

        Self { nodes, queue }
    }
}

impl<'trace> Iterator for CallTraceNodeWalkerBF<'trace> {
    type Item = &'trace CallTraceNode;

    fn next(&mut self) -> Option<Self::Item> {
        match self.queue.pop_front() {
            Some(idx) => {
                let curr = self.nodes.get(idx).expect("missing node");

                for child_idx in curr.children.iter() {
                    self.queue.push_back(*child_idx);
                }

                Some(curr)
            }
            None => None,
        }
    }
}

/// lazy iterator over a vm trace breadth first
// pub(crate) struct VmTraceWalkerBF<'trace> {
//     queue: VecDeque<&'trace mut VmTrace>,
// }

// impl<'trace> VmTraceWalkerBF<'trace> {
//     pub(crate) fn new(trace: &'trace mut VmTrace) -> Self {
//         let mut queue = VecDeque::new();
//         queue.push_back(trace);

//         Self { queue }
//     }
// }

// impl<'trace> Iterator for VmTraceWalkerBF<'trace> {
//     type Item = &'trace mut VmTrace;

//     fn next(&mut self) -> Option<Self::Item> {
//         if let Some(trace) = self.queue.pop_front() {
//             let ops = &mut trace.ops;

//             for child in ops.iter_mut() {
//                 match child.sub {
//                     Some(ref mut sub) => self.queue.push_back(sub),
//                     None => {}
//                 }
//             }

//             return Some(trace);
//         } else {
//             return None;
//         }
//     }
// }

#[cfg(test)]
mod tests {
    use crate::tracing::builder::walker::CallTraceNodeWalkerDF;
    use crate::tracing::types::CallTraceNode;
    use crate::tracing::TracingInspectorConfig;

    #[test]
    fn test_walker_build() {
        let nodes = vec![
            CallTraceNode { idx: 0, parent: None, children: vec![1, 2], ..Default::default() },
            CallTraceNode { idx: 1, parent: Some(0), children: vec![3], ..Default::default() },
            CallTraceNode { idx: 2, parent: Some(0), children: vec![4], ..Default::default() },
            CallTraceNode { idx: 3, parent: Some(1), children: vec![], ..Default::default() },
            CallTraceNode { idx: 4, parent: Some(2), children: vec![], ..Default::default() },
        ];

        let mut walker = CallTraceNodeWalkerDF::new(&nodes);
        // println!("{:#?}", walker);

        assert_eq!(walker.next().unwrap().clone(), nodes[0]);
        assert_eq!(walker.next().unwrap().clone(), nodes[1]);
        assert_eq!(walker.next().unwrap().clone(), nodes[3]);
        assert_eq!(walker.next().unwrap().clone(), nodes[2]);
        assert_eq!(walker.next().unwrap().clone(), nodes[4]);
    }
}
