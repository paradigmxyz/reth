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

impl<'trace> CallTraceNodeWalker<'trace, DFWalk> {
    pub(crate) fn new(nodes: &'trace Vec<CallTraceNode>) -> Self {
        let mut idxs: Vec<usize> = Vec::with_capacity(nodes.len());

        Self::get_all_children(nodes, &mut idxs, 0);

        Self { nodes, curr_idx: 0, idxs, phantom: std::marker::PhantomData }
    }

    /// uses recursion to do a DFS down the callgraph
    fn get_all_children(nodes: &'trace Vec<CallTraceNode>, holder: &mut Vec<usize>, idx: usize) {
        holder.push(idx);
        for child_idx in nodes[idx].children.iter() {
            Self::get_all_children(nodes, holder, *child_idx);
        }
    }

    /// DFWalked order of input arena
    pub(crate) fn idxs(&self) -> &Vec<usize> {
        &self.idxs
    }
}

impl<'trace> Iterator for CallTraceNodeWalker<'trace, DFWalk> {
    type Item = &'trace CallTraceNode;

    fn next(&mut self) -> Option<Self::Item> {
        if !(self.curr_idx < self.nodes.len()) {
            return None;
        }

        let node = &self.nodes[self.idxs[self.curr_idx]];
        self.curr_idx += 1;

        Some(node)
    }
}

#[cfg(test)]
mod tests {
    use crate::tracing::builder::walker::{CallTraceNodeWalker, DFWalk};
    use crate::tracing::types::{CallTrace, CallTraceNode};

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

        let mut i = walker.idxs().len();
        for (idx, node) in walker.enumerate() {
            i += 1;
        }
    }
}
