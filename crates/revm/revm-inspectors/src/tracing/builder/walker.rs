use crate::tracing::types::CallTraceNode;
use std::collections::VecDeque;

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
                let curr = self.nodes.get(idx).expect("there should be a node");

                self.queue.extend(curr.children.iter());

                Some(curr)
            }
            None => None,
        }
    }
}
