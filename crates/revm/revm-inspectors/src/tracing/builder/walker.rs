use crate::tracing::types::CallTraceNode;
use reth_primitives::Address;
use reth_rpc_types::trace::parity::VmTrace;

/// special type for doing a Depth first walk down the callgraph
pub(crate) struct DF;

trait Walker<O>: Iterator<Item = Self::Node> {
    type Node;

    fn len(&self) -> usize;
}

/// pub crate type for doing a Depth first walk down the callgraph
pub(crate) struct CallTraceNodeWalker<'a, O> {
    /// the entire arena
    nodes: Vec<&'a CallTraceNode>,
    curr_idx: usize,
    /// ordered indexes of [nodes]
    idxs: Vec<usize>,
    phantom: std::marker::PhantomData<O>,
}

/// pub crate type for doing a Depth first walk down the callgraph
pub(crate) struct VmTraceWalker<'a, O> {
    curr_idx: usize,
    /// ordered set of nodes
    nodes: Vec<&'a VmTrace>,
    phantom: std::marker::PhantomData<O>,
}

impl<'a> VmTraceWalker<'a, DF> {
    fn get_all_subs(node: &'a VmTrace, holder: &mut Vec<&'a VmTrace>) {
        holder.push(node);

        for op in node.ops.iter() {
            if let Some(sub) = &op.sub {
                Self::get_all_subs(sub, holder);
            }
        }
    }
}

impl<'a> CallTraceNodeWalker<'a, DF> {
    fn get_all_children(nodes: &Vec<&'a CallTraceNode>, idx: usize, holder: &mut Vec<usize>) {
        for child in nodes[idx].children.iter() {
            holder.push(child.clone());
            Self::get_all_children(nodes, *child, holder);
        }
    }

    pub(crate) fn DF_addresses(&self) -> Vec<Address> {
        self.idxs.iter().map(|idx| self.nodes[*idx].trace.address).collect::<Vec<_>>()
    }
}

impl<'a> Iterator for VmTraceWalker<'a, DF> {
    type Item = &'a VmTrace;

    fn next(&mut self) -> Option<Self::Item> {
        if self.curr_idx >= self.nodes.len() {
            return None;
        }

        let node = self.nodes[self.curr_idx];
        self.curr_idx += 1;

        Some(node)
    }
}

impl<'a> Iterator for CallTraceNodeWalker<'a, DF> {
    type Item = &'a CallTraceNode;

    fn next(&mut self) -> Option<Self::Item> {
        if self.curr_idx >= self.nodes.len() {
            return None;
        }

        let node = self.nodes[self.idxs[self.curr_idx]];
        self.curr_idx += 1;

        Some(node)
    }
}

impl<'a> From<Vec<&'a CallTraceNode>> for CallTraceNodeWalker<'a, DF> {
    fn from(nodes: Vec<&'a CallTraceNode>) -> Self {
        let mut idxs: Vec<usize> = Vec::with_capacity(nodes.len());

        Self::get_all_children(&nodes, 0, &mut idxs);

        Self { nodes, curr_idx: 0, idxs, phantom: std::marker::PhantomData }
    }
}

impl<'a> From<&'a VmTrace> for VmTraceWalker<'a, DF> {
    fn from(node: &'a VmTrace) -> Self {
        let mut nodes: Vec<&'a VmTrace> = Vec::new();

        Self::get_all_subs(node, &mut nodes);

        Self { curr_idx: 0, nodes, phantom: std::marker::PhantomData }
    }
}

impl<'a> Walker<DF> for VmTraceWalker<'a, DF> {
    type Node = &'a VmTrace;

    fn len(&self) -> usize {
        self.nodes.len()
    }
}

impl<'a> Walker<DF> for CallTraceNodeWalker<'a, DF> {
    type Node = &'a CallTraceNode;

    fn len(&self) -> usize {
        self.nodes.len()
    }
}
