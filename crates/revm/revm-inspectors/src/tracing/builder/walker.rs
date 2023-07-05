use crate::tracing::{types::CallTraceStep, TracingInspectorConfig};
use reth_primitives::Address;
use reth_rpc_types::trace::parity::{
    MemoryDelta, StorageDelta, VmExecutedOperation, VmInstruction, VmTrace,
};
use revm::interpreter::opcode;
use std::collections::VecDeque;

use crate::tracing::types::CallTraceNode;

/// Traverses Reths internal tracing structure breadth-first
///
/// This is a lazy iterator
pub(crate) struct CallTraceNodeWalkerBF<'trace> {
    /// the entire arena
    nodes: &'trace Vec<CallTraceNode>,

    /// holds indexes of nodes to visit as we traverse
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

// // use reth_rpc_types::trace::parity::VmTrace;

// /// Traverses Reths internal tracing structure depth-first
// ///
// /// This is a lazy iterator, so it will only traverse the tree as you call next()
// #[derive(Debug, Clone)]
// pub(crate) struct CallTraceNodeWalkerDF<'trace> {
//     /// the entire callgraph or arena
//     nodes: &'trace Vec<CallTraceNode>,

//     /// the next idx to check if visited or return
//     curr_idx: usize,

//     /// holds the offset into the children array of the nodes parents
//     child_idx_stack: Vec<usize>,

//     /// keeps track of which nodes we've visited
//     visited: Vec<bool>,
// }

// impl<'trace> CallTraceNodeWalkerDF<'trace> {
//     pub(crate) fn new(nodes: &'trace Vec<CallTraceNode>) -> Self {
//         Self {
//             nodes,
//             curr_idx: 0,
//             /// can never be deeper than len of arena
//             child_idx_stack: Vec::with_capacity(nodes.len()),
//             visited: vec![false; nodes.len()],
//         }
//     }
// }

// impl<'trace> Iterator for CallTraceNodeWalkerDF<'trace> {
//     type Item = &'trace CallTraceNode;

//     fn next(&mut self) -> Option<Self::Item> {
//         let mut next_child = 0;
//         loop {
//             if !self.visited[self.curr_idx] {
//                 self.visited[self.curr_idx] = true;
//                 return self.nodes.get(self.curr_idx);
//             }

//             let curr = self.nodes.get(self.curr_idx).expect("missing node");

//             match curr.children.get(next_child) {
//                 Some(child_idx) => {
//                     self.child_idx_stack.push(next_child + 1);

//                     self.curr_idx = *child_idx;
//                 }
//                 None => match curr.parent {
//                     Some(parent_idx) => {
//                         next_child = self.child_idx_stack.pop().expect("missing stack");
//                         self.curr_idx = parent_idx;
//                     }
//                     None => {
//                         return None;
//                     }
//                 },
//             }
//         }
//     }
// }

// #[cfg(test)]
// mod tests {
//     use crate::tracing::types::CallTraceNode;
//     use crate::tracing::TracingInspectorConfig;

//     #[test]
//     fn test_walker_build() {
//         let nodes = vec![
//             CallTraceNode { idx: 0, parent: None, children: vec![1, 2], ..Default::default() },
//             CallTraceNode { idx: 1, parent: Some(0), children: vec![3], ..Default::default() },
//             CallTraceNode { idx: 2, parent: Some(0), children: vec![4], ..Default::default() },
//             CallTraceNode { idx: 3, parent: Some(1), children: vec![], ..Default::default() },
//             CallTraceNode { idx: 4, parent: Some(2), children: vec![], ..Default::default() },
//         ];

//         let mut walker = CallTraceNodeWalkerDF::new(&nodes);
//         // println!("{:#?}", walker);

//         assert_eq!(walker.next().unwrap().clone(), nodes[0]);
//         assert_eq!(walker.next().unwrap().clone(), nodes[1]);
//         assert_eq!(walker.next().unwrap().clone(), nodes[3]);
//         assert_eq!(walker.next().unwrap().clone(), nodes[2]);
//         assert_eq!(walker.next().unwrap().clone(), nodes[4]);
//     }
// }
