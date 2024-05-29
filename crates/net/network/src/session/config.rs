//! Configuration types for [SessionManager](crate::session::SessionManager).

use crate::{
    peers::{DEFAULT_MAX_COUNT_PEERS_INBOUND, DEFAULT_MAX_COUNT_PEERS_OUTBOUND},
    session::{Direction, ExceedsSessionLimit},
};
use std::time::Duration;

/// Default request timeout for a single request.
///
/// This represents the amount of time we wait for a response until we consider it timed out.
pub const INITIAL_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Default timeout after which a pending session attempt is considered failed.
pub const PENDING_SESSION_TIMEOUT: Duration = Duration::from_secs(20);

/// Default timeout after which we'll consider the peer to be in violation of the protocol.
///
/// This is the time a peer has to answer a response.
pub const PROTOCOL_BREACH_REQUEST_TIMEOUT: Duration = Duration::from_secs(2 * 60);

/// The default maximum number of peers.
const DEFAULT_MAX_PEERS: usize =
    DEFAULT_MAX_COUNT_PEERS_OUTBOUND as usize + DEFAULT_MAX_COUNT_PEERS_INBOUND as usize;

/// The default session event buffer size.
///
/// The actual capacity of the event channel will be `buffer + num sessions`.
/// With maxed out peers, this will allow for 3 messages per session (average)
const DEFAULT_SESSION_EVENT_BUFFER_SIZE: usize = DEFAULT_MAX_PEERS * 2;

/// Configuration options when creating a [SessionManager](crate::session::SessionManager).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct SessionsConfig {
    /// Size of the session command buffer (per session task).
    pub session_command_buffer: usize,
    /// Size of the session event channel buffer.
    pub session_event_buffer: usize,
    /// Limits to enforce.
    ///
    /// By default, no limits will be enforced.
    pub limits: SessionLimits,
    /// The maximum initial time we wait for a response from the peer before we timeout a request
    /// _internally_.
    pub initial_internal_request_timeout: Duration,
    /// The amount of time we continue to wait for a response from the peer, even if we timed it
    /// out internally (`initial_internal_request_timeout`). Timeouts are not penalized but the
    /// session directly, however if a peer fails to respond at all (within
    /// `PROTOCOL_BREACH_REQUEST_TIMEOUT`) this is considered a protocol violation and results in a
    /// dropped session.
    pub protocol_breach_request_timeout: Duration,
    /// The timeout after which a pending session attempt is considered failed.
    pub pending_session_timeout: Duration,
}

impl Default for SessionsConfig {
    fn default() -> Self {
        Self {
            // This should be sufficient to slots for handling commands sent to the session task,
            // since the manager is the sender.
            session_command_buffer: 32,
            // This should be greater since the manager is the receiver. The total size will be
            // `buffer + num sessions`. Each session can therefore fit at least 1 message in the
            // channel. The buffer size is additional capacity. The channel is always drained on
            // `poll`.
            // The default is twice the maximum number of available slots, if all slots are occupied
            // the buffer will have capacity for 3 messages per session (average).
            session_event_buffer: DEFAULT_SESSION_EVENT_BUFFER_SIZE,
            limits: Default::default(),
            initial_internal_request_timeout: INITIAL_REQUEST_TIMEOUT,
            protocol_breach_request_timeout: PROTOCOL_BREACH_REQUEST_TIMEOUT,
            pending_session_timeout: PENDING_SESSION_TIMEOUT,
        }
    }
}

impl SessionsConfig {
    /// Sets the buffer size for the bounded communication channel between the manager and its
    /// sessions for events emitted by the sessions.
    ///
    /// It is expected, that the background session task will stall if they outpace the manager. The
    /// buffer size provides backpressure on the network I/O.
    pub const fn with_session_event_buffer(mut self, n: usize) -> Self {
        self.session_event_buffer = n;
        self
    }

    /// Helper function to set the buffer size for the bounded communication channel between the
    /// manager and its sessions for events emitted by the sessions.
    ///
    /// This scales the buffer size based on the configured number of peers, where the base line is
    /// the default buffer size.
    ///
    /// If the number of peers is greater than the default, the buffer size will be scaled up to
    /// match the default `buffer size / max peers` ratio.
    ///
    /// Note: This is capped at 10 times the default buffer size.
    pub fn with_upscaled_event_buffer(mut self, num_peers: usize) -> Self {
        if num_peers > DEFAULT_MAX_PEERS {
            self.session_event_buffer = (num_peers * 2).min(DEFAULT_SESSION_EVENT_BUFFER_SIZE * 10);
        }
        self
    }
}

/// Limits for sessions.
///
/// By default, no session limits will be enforced
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SessionLimits {
    max_pending_inbound: Option<u32>,
    max_pending_outbound: Option<u32>,
    max_established_inbound: Option<u32>,
    max_established_outbound: Option<u32>,
}

impl SessionLimits {
    /// Sets the maximum number of pending incoming sessions.
    pub const fn with_max_pending_inbound(mut self, limit: u32) -> Self {
        self.max_pending_inbound = Some(limit);
        self
    }

    /// Sets the maximum number of pending outbound sessions.
    pub const fn with_max_pending_outbound(mut self, limit: u32) -> Self {
        self.max_pending_outbound = Some(limit);
        self
    }

    /// Sets the maximum number of active inbound sessions.
    pub const fn with_max_established_inbound(mut self, limit: u32) -> Self {
        self.max_established_inbound = Some(limit);
        self
    }

    /// Sets the maximum number of active outbound sessions.
    pub const fn with_max_established_outbound(mut self, limit: u32) -> Self {
        self.max_established_outbound = Some(limit);
        self
    }
}

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

    #[test]
    fn scale_session_event_buffer() {
        let config = SessionsConfig::default().with_upscaled_event_buffer(10);
        assert_eq!(config.session_event_buffer, DEFAULT_SESSION_EVENT_BUFFER_SIZE);
        let default_ration = config.session_event_buffer / DEFAULT_MAX_PEERS;

        let config = SessionsConfig::default().with_upscaled_event_buffer(DEFAULT_MAX_PEERS * 2);
        let expected_ration = config.session_event_buffer / (DEFAULT_MAX_PEERS * 2);
        assert_eq!(default_ration, expected_ration);
    }
}
