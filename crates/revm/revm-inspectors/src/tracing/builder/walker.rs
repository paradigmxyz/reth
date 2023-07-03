use hashbrown::HashMap;

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

                            // we are done with this child, so we go to the next one
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
    pub(crate) fn idxs(&self) -> &Vec<usize> {
        &self.idxs
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
