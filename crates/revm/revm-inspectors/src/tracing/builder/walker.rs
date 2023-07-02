use crate::tracing::types::CallTraceNode;
// use reth_rpc_types::trace::parity::VmTrace;

/// type for doing a Depth first walk down the callgraph
pub(crate) struct DFWalk;

/// pub crate type for doing a walk down a reth callgraph
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

        Self::get_all_children(nodes, 0, &mut idxs);

        Self { nodes, curr_idx: 0, idxs, phantom: std::marker::PhantomData }
    }

    /// DFWalked order of input arena
    pub(crate) fn idxs(&self) -> &Vec<usize> {
        &self.idxs
    }

    fn get_all_children(nodes: &'trace Vec<CallTraceNode>, idx: usize, holder: &mut Vec<usize>) {
        holder.push(idx);
        for child in nodes[idx].children.iter() {
            holder.push(child.clone());
            Self::get_all_children(nodes, *child, holder);
        }
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
