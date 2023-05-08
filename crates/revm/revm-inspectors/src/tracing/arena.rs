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
    pub(crate) fn push_trace(&mut self, entry: usize, new_trace: CallTrace) -> usize {
        match new_trace.depth {
            // The entry node, just update it
            0 => {
                self.arena[0].trace = new_trace;
                0
            }
            // We found the parent node, add the new trace as a child
            _ if self.arena[entry].trace.depth == new_trace.depth - 1 => {
                let id = self.arena.len();

                let trace_location = self.arena[entry].children.len();
                self.arena[entry].ordering.push(LogCallOrder::Call(trace_location));
                let node = CallTraceNode {
                    parent: Some(entry),
                    trace: new_trace,
                    idx: id,
                    ..Default::default()
                };
                self.arena.push(node);
                self.arena[entry].children.push(id);

                id
            }
            // We haven't found the parent node, go deeper
            _ => self.push_trace(
                *self.arena[entry].children.last().expect("Disconnected trace"),
                new_trace,
            ),
        }
    }
}

impl Default for CallTraceArena {
    fn default() -> Self {
        // The first node is the root node
        CallTraceArena { arena: vec![Default::default()] }
    }
}
