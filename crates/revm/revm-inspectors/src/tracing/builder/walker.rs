use crate::tracing::types::CallTraceNode;
// use reth_rpc_types::trace::parity::VmTrace;

/// type for doing a Depth first walk down the callgraph
pub(crate) struct DFWalk;

pub(crate) trait Walk {}

impl Walk for DFWalk {}

pub(crate) trait Walker<T, W: Walk>: Iterator<Item = T> {}

/// pub crate type for doing a walk down a reth callgraph
pub(crate) struct CallTraceNodeWalker<'trace, W: Walk> {
    /// the entire arena
    nodes: &'trace Vec<CallTraceNode>,
    curr_idx: usize,

    /// ordered indexes of nodes
    idxs: Vec<usize>,

    phantom: std::marker::PhantomData<W>,
}

impl<'trace> CallTraceNodeWalker<'trace, DFWalk> {
    fn get_all_children(nodes: &'trace Vec<CallTraceNode>, idx: usize, holder: &mut Vec<usize>) {
        holder.push(idx);
        for child in nodes[idx].children.iter() {
            holder.push(child.clone());
            Self::get_all_children(nodes, *child, holder);
        }
    }

    /// DFWalked order of input arena
    pub(crate) fn idxs(&self) -> &Vec<usize> {
        &self.idxs
    }
}

impl<'trace> From<&'trace Vec<CallTraceNode>> for CallTraceNodeWalker<'trace, DFWalk> {
    fn from(nodes: &'trace Vec<CallTraceNode>) -> Self {
        let mut idxs: Vec<usize> = Vec::with_capacity(nodes.len());

        Self::get_all_children(nodes, 0, &mut idxs);

        Self { nodes, curr_idx: 0, idxs, phantom: std::marker::PhantomData }
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

impl<'trace> Walker<&'trace CallTraceNode, DFWalk> for CallTraceNodeWalker<'trace, DFWalk> {}

// /// pub crate type for doing a walk down a parity callgraph
// pub(crate) struct VmTraceWalker<'trace, W: Walk> {
//     root: VmTrace,
//     parent: Option<&'trace mut VmTrace>,
//     current: Option<&'trace mut VmTrace>,

//     /// the currnet index of the sub call on this level
//     idx: usize,

//     done: bool,
//     len: usize,

//     phantom: std::marker::PhantomData<W>,
// }

// impl<'trace> VmTraceWalker<'trace, DFWalk> {
//     fn maybe_next_sub(
//         trace: &'trace mut VmTrace,
//         idx: usize,
//     ) -> Option<(usize, &'trace mut VmTrace)> {
//         for (curr, op) in trace.ops.iter_mut().enumerate() {
//             if curr > idx {
//                 return op.sub.as_mut().map(|sub| (curr, sub));
//             }
//         }

//         None
//     }
// }

// impl<'trace> Iterator for VmTraceWalker<'trace, DFWalk> {
//     type Item = &'trace mut VmTrace;

//     fn next(&mut self) -> Option<Self::Item> {
//         match self.current {
//             Some(trace) => {
//                 if let Some((new_idx, new_trace)) = Self::maybe_next_sub(trace, self.idx) {
//                     self.idx = new_idx;
//                     self.parent = self.current;
//                     self.current = Some(new_trace);
//                     Some(new_trace)
//                 } else {
//                     self.current = self.parent;
//                     self.idx = 0;
//                     // self.next()
//                     None
//                 }
//             }
//             None => {
//                 self.current = Some(&mut self.root);
//                 Some(&mut self.root)
//             }
//         }
//     }
// }

// impl<'trace> From<VmTrace> for VmTraceWalker<'trace, DFWalk> {
//     fn from(node: VmTrace) -> Self {
//         todo!()
//     }
// }

// impl<'trace> Walker<&'trace mut VmTrace, DFWalk> for VmTraceWalker<'trace, DFWalk> {
//     fn len(&self) -> usize {
//         todo!()
//     }
// }
