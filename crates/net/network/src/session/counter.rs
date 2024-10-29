use reth_network_api::Direction;
use reth_network_types::SessionLimits;

use super::ExceedsSessionLimit;

/// Keeps track of all sessions.
#[derive(Debug, Clone)]
pub struct SessionCounter {
    /// Limits to enforce.
    limits: SessionLimits,
    /// Number of pending incoming sessions.
    pending_inbound: u32,
    /// Number of pending outgoing sessions.
    pending_outbound: u32,
    /// Number of active inbound sessions.
    active_inbound: u32,
    /// Number of active outbound sessions.
    active_outbound: u32,
}

// === impl SessionCounter ===

impl SessionCounter {
    pub(crate) const fn new(limits: SessionLimits) -> Self {
        Self {
            limits,
            pending_inbound: 0,
            pending_outbound: 0,
            active_inbound: 0,
            active_outbound: 0,
        }
    }

    pub(crate) fn inc_pending_inbound(&mut self) {
        self.pending_inbound += 1;
    }

    pub(crate) fn inc_pending_outbound(&mut self) {
        self.pending_outbound += 1;
    }

    pub(crate) fn dec_pending(&mut self, direction: &Direction) {
        match direction {
            Direction::Outgoing(_) => {
                self.pending_outbound -= 1;
            }
            Direction::Incoming => {
                self.pending_inbound -= 1;
            }
        }
    }

    pub(crate) fn inc_active(&mut self, direction: &Direction) {
        match direction {
            Direction::Outgoing(_) => {
                self.active_outbound += 1;
            }
            Direction::Incoming => {
                self.active_inbound += 1;
            }
        }
    }

    pub(crate) fn dec_active(&mut self, direction: &Direction) {
        match direction {
            Direction::Outgoing(_) => {
                self.active_outbound -= 1;
            }
            Direction::Incoming => {
                self.active_inbound -= 1;
            }
        }
    }

    pub(crate) const fn ensure_pending_outbound(&self) -> Result<(), ExceedsSessionLimit> {
        Self::ensure(self.pending_outbound, self.limits.max_pending_outbound)
    }

    pub(crate) const fn ensure_pending_inbound(&self) -> Result<(), ExceedsSessionLimit> {
        Self::ensure(self.pending_inbound, self.limits.max_pending_inbound)
    }

    const fn ensure(current: u32, limit: Option<u32>) -> Result<(), ExceedsSessionLimit> {
        if let Some(limit) = limit {
            if current >= limit {
                return Err(ExceedsSessionLimit(limit))
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_limits() {
        let mut limits = SessionCounter::new(SessionLimits::default().with_max_pending_inbound(2));
        assert!(limits.ensure_pending_outbound().is_ok());
        limits.inc_pending_inbound();
        assert!(limits.ensure_pending_inbound().is_ok());
        limits.inc_pending_inbound();
        assert!(limits.ensure_pending_inbound().is_err());
    }
}
