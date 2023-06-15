use crate::tracing::types::{CallTrace, CallTraceNode, LogCallOrder};

/// An arena of recorded traces.
///
/// This type will be populated via the [TracingInspector](crate::tracing::TracingInspector).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallTraceArena {
    /// The arena of recorded trace nodes
    pub(crate) arena: Vec<CallTraceNode>,
}

impl CallTraceArena {
    /// Pushes a new trace into the arena, returning the trace ID
    ///
    /// This appends a new trace to the arena, and also inserts a new entry in the node's parent
    /// node children set if `attach_to_parent` is `true`. E.g. if calls to precompiles should
    /// not be included in the call graph this should be called with [PushTraceKind::PushOnly].
    pub(crate) fn push_trace(
        &mut self,
        mut entry: usize,
        kind: PushTraceKind,
        new_trace: CallTrace,
    ) -> usize {
        loop {
            match new_trace.depth {
                // The entry node, just update it
                0 => {
                    self.arena[0].trace = new_trace;
                    return 0
                }
                // We found the parent node, add the new trace as a child
                _ if self.arena[entry].trace.depth == new_trace.depth - 1 => {
                    let id = self.arena.len();
                    let node = CallTraceNode {
                        parent: Some(entry),
                        trace: new_trace,
                        idx: id,
                        ..Default::default()
                    };
                    self.arena.push(node);

                    // also track the child in the parent node
                    if kind.is_attach_to_parent() {
                        let parent = &mut self.arena[entry];
                        let trace_location = parent.children.len();
                        parent.ordering.push(LogCallOrder::Call(trace_location));
                        parent.children.push(id);
                    }

                    return id
                }
                _ => {
                    // We haven't found the parent node, go deeper
                    entry = *self.arena[entry].children.last().expect("Disconnected trace");
                }
            }
        }
    }
}

/// How to push a trace into the arena
pub(crate) enum PushTraceKind {
    /// This will _only_ push the trace into the arena.
    PushOnly,
    /// This will push the trace into the arena, and also insert a new entry in the node's parent
    /// node children set.
    PushAndAttachToParent,
}

impl PushTraceKind {
    fn is_attach_to_parent(&self) -> bool {
        matches!(self, PushTraceKind::PushAndAttachToParent)
    }
}

impl Default for CallTraceArena {
    fn default() -> Self {
        // The first node is the root node
        CallTraceArena { arena: vec![Default::default()] }
    }
}
