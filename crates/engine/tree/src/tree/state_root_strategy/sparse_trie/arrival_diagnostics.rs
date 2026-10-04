//! Opt-in consumer-side observations; these are wall intervals, not CPU or causal attribution.
//!
//! Crossbeam's is_empty observes reserved channel positions before message publication. A positive
//! pre-select observation therefore establishes channel nonemptiness, not a readable proof. The
//! nonempty lower intervals and selection counts cannot establish proof-ready dwell or starvation.
//! Empty observations still anchor conservative upper bounds until first proof service. Bounds
//! describe channel episodes, not individual proof residence: service coalesces concurrent
//! arrivals. State counts are received canonical messages, including empty or subsequently
//! coalesced updates; they are not counts of distinct trie leaves. Finish snapshots also retain
//! buffered map sizes.

use reth_primitives_traits::FastInstant as Instant;

pub(super) struct ArrivalDiagnostics {
    started: Instant,
    messages: [u64; 3],
    nonempty_selections: [u64; 3],
    pub(super) pending_state: u64,
    pending_hints: u64,
    pending_state_max: u64,
    oldest_state: Option<u64>,
    first_state: Option<u64>,
    last_state: Option<u64>,
    pub(super) flushes: [u64; 4],
    pub(super) flush_reason: Option<FlushReason>,
    pub(super) flushed_state: u64,
    pub(super) flushed_hints: u64,
    oldest_state_age_max: u64,
    first_flush: Option<u64>,
    last_flush: Option<u64>,
    pub(super) deferred_skips: u64,
    pub(super) deferred_episodes: u64,
    deferred_since: Option<u64>,
    deferred_ns: u64,
    deferred_max_ns: u64,
    proof_empty_before: Option<u64>,
    proof_channel_nonempty_since: Option<u64>,
    proof_services: u64,
    proof_upper_unknown: u64,
    proof_nonempty_lower_ns: u64,
    proof_nonempty_lower_max_ns: u64,
    proof_upper_ns: u64,
    proof_upper_max_ns: u64,
    pub(super) proof_messages: u64,
    select_wait_ns: [u64; 2],
    finish_at: Option<u64>,
    finish: FinishSnapshot,
}

impl ArrivalDiagnostics {
    pub(super) fn new() -> Self {
        Self {
            started: Instant::now(),
            messages: [0; 3],
            nonempty_selections: [0; 3],
            pending_state: 0,
            pending_hints: 0,
            pending_state_max: 0,
            oldest_state: None,
            first_state: None,
            last_state: None,
            flushes: [0; 4],
            flush_reason: None,
            flushed_state: 0,
            flushed_hints: 0,
            oldest_state_age_max: 0,
            first_flush: None,
            last_flush: None,
            deferred_skips: 0,
            deferred_episodes: 0,
            deferred_since: None,
            deferred_ns: 0,
            deferred_max_ns: 0,
            proof_empty_before: None,
            proof_channel_nonempty_since: None,
            proof_services: 0,
            proof_upper_unknown: 0,
            proof_nonempty_lower_ns: 0,
            proof_nonempty_lower_max_ns: 0,
            proof_upper_ns: 0,
            proof_upper_max_ns: 0,
            proof_messages: 0,
            select_wait_ns: [0; 2],
            finish_at: None,
            finish: FinishSnapshot::default(),
        }
    }

    pub(super) fn offset(&self, instant: Instant) -> u64 {
        instant.duration_since(self.started).as_nanos().min(u64::MAX as u128) as u64
    }

    pub(super) fn now(&self) -> u64 {
        self.offset(Instant::now())
    }

    pub(super) fn message(&mut self, kind: MessageKind, at: u64, channel_nonempty: bool) {
        let index = kind as usize;
        self.messages[index] += 1;
        self.nonempty_selections[index] += u64::from(channel_nonempty);
        match kind {
            MessageKind::State => {
                self.pending_state += 1;
                self.pending_state_max = self.pending_state_max.max(self.pending_state);
                self.oldest_state.get_or_insert(at);
                self.first_state.get_or_insert(at);
                self.last_state = Some(at);
            }
            MessageKind::Hint => self.pending_hints += 1,
            MessageKind::Finish => self.finish_at = Some(at),
        }
    }

    pub(super) fn finish(&mut self, mut snapshot: FinishSnapshot, at: u64) {
        snapshot.state = self.pending_state;
        snapshot.hints = self.pending_hints;
        snapshot.oldest_state_age_ns = self.oldest_state.map_or(0, |start| at - start);
        self.finish = snapshot;
    }

    pub(super) fn defer(&mut self, at: u64) {
        if self.pending_state == 0 {
            return;
        }
        self.deferred_skips += 1;
        if self.deferred_since.is_none() {
            self.deferred_since = Some(at);
            self.deferred_episodes += 1;
        }
    }

    // Called exactly where pending_updates is reset, before fallible leaf processing. These are
    // flush attempts; a returned error must remain visible in the final summary.
    pub(super) fn flush(&mut self, at: u64) {
        self.flushes[self.flush_reason.take().map_or(3, |reason| reason as usize)] += 1;
        self.first_flush.get_or_insert(at);
        self.last_flush = Some(at);
        self.flushed_state += self.pending_state;
        self.flushed_hints += self.pending_hints;
        self.oldest_state_age_max =
            self.oldest_state_age_max.max(self.oldest_state.take().map_or(0, |start| at - start));
        self.pending_state = 0;
        self.pending_hints = 0;
        if let Some(start) = self.deferred_since.take() {
            let duration = at - start;
            self.deferred_ns += duration;
            self.deferred_max_ns = self.deferred_max_ns.max(duration);
        }
    }

    // Before an empty observation anchors a conservative upper bound. After a positive observation
    // starts only a reserved-channel nonempty lower bound; message publication is not observed.
    // A timestamp after Empty could miss a new reservation.
    pub(super) fn observe_proofs(
        &mut self,
        before: u64,
        after: u64,
        channel_nonempty: bool,
    ) -> Selection {
        if channel_nonempty {
            self.proof_channel_nonempty_since.get_or_insert(after);
        } else {
            self.proof_empty_before = Some(before);
            self.proof_channel_nonempty_since = None;
        }
        Selection { proof_channel_nonempty: channel_nonempty, before_select: after }
    }

    pub(super) fn proof_service(&mut self, selection: Selection, received: u64) {
        self.proof_services += 1;
        let lower = self
            .proof_channel_nonempty_since
            .take()
            .map_or(0, |start| selection.before_select - start);
        self.proof_nonempty_lower_ns += lower;
        self.proof_nonempty_lower_max_ns = self.proof_nonempty_lower_max_ns.max(lower);
        if let Some(start) = self.proof_empty_before.take() {
            let upper = received - start;
            self.proof_upper_ns += upper;
            self.proof_upper_max_ns = self.proof_upper_max_ns.max(upper);
        } else {
            self.proof_upper_unknown += 1;
        }
    }

    pub(super) fn proofs_drained(&mut self, receive_started: u64) {
        // try_recv reached Empty sometime after receive_started. Its exact instant is unknown.
        self.proof_empty_before = Some(receive_started);
        self.proof_channel_nonempty_since = None;
    }

    pub(super) fn select_wait(&mut self, draining: bool, before: Instant, wake: Instant) {
        self.select_wait_ns[usize::from(draining)] += self.offset(wake) - self.offset(before);
    }

    pub(super) fn report(&self, status: &'static str, error: Option<&dyn std::fmt::Debug>) {
        let elapsed_ns = self.now();
        tracing::trace!(target: "engine::root::arrival",
            schema = 1u64, status, ?error, elapsed_ns,
            readiness_publication_observed = false,
            state_messages = self.messages[0], hint_messages = self.messages[1],
            finish_messages = self.messages[2],
            first_state_ns = self.first_state.unwrap_or(0), last_state_ns = self.last_state.unwrap_or(0),
            first_flush_ns = self.first_flush.unwrap_or(0), last_flush_ns = self.last_flush.unwrap_or(0),
            flush_initial_threshold = self.flushes[0], flush_input_gap = self.flushes[1],
            flush_finished = self.flushes[2], flush_unclassified = self.flushes[3],
            flushed_state = self.flushed_state, flushed_hints = self.flushed_hints,
            pending_state = self.pending_state, pending_hints = self.pending_hints,
            pending_oldest_state_age_ns = self.oldest_state.map_or(0, |start| elapsed_ns - start),
            pending_state_max = self.pending_state_max, oldest_state_age_max_ns = self.oldest_state_age_max,
            deferred_skips = self.deferred_skips, deferred_episodes = self.deferred_episodes,
            deferred_ns = self.deferred_ns, deferred_max_ns = self.deferred_max_ns,
            deferred_open = self.deferred_since.is_some(),
            deferred_open_ns = self.deferred_since.map_or(0, |start| elapsed_ns - start),
            proof_nonempty_state_selections = self.nonempty_selections[0], proof_nonempty_hint_selections = self.nonempty_selections[1],
            proof_nonempty_finish_selections = self.nonempty_selections[2], proof_services = self.proof_services,
            proof_messages = self.proof_messages, proof_upper_unknown = self.proof_upper_unknown,
            proof_nonempty_lower_ns = self.proof_nonempty_lower_ns, proof_nonempty_lower_max_ns = self.proof_nonempty_lower_max_ns,
            proof_upper_ns = self.proof_upper_ns, proof_upper_max_ns = self.proof_upper_max_ns,
            proof_channel_nonempty_open = self.proof_channel_nonempty_since.is_some(),
            proof_channel_nonempty_open_ns = self.proof_channel_nonempty_since.map_or(0, |start| elapsed_ns - start),
            streaming_select_wait_ns = self.select_wait_ns[0], draining_select_wait_ns = self.select_wait_ns[1],
            finish_ns = self.finish_at.unwrap_or(0), finish_state = self.finish.state,
            finish_hints = self.finish.hints, finish_oldest_state_age_ns = self.finish.oldest_state_age_ns,
            finish_accounts = self.finish.accounts, finish_storage_accounts = self.finish.storage_accounts,
            finish_targets = self.finish.targets, finish_proofs_in_flight = self.finish.proofs_in_flight,
            finish_storage_in_flight = self.finish.storage_in_flight, finish_proof_channel_nonempty = self.finish.proof_channel_nonempty,
            "Sparse trie arrival summary");
    }
}

#[derive(Clone, Copy)]
pub(super) enum MessageKind {
    State,
    Hint,
    Finish,
}

#[derive(Clone, Copy)]
pub(super) enum FlushReason {
    InitialThreshold,
    InputGap,
    Finished,
}

#[derive(Clone, Copy)]
pub(super) struct Selection {
    pub(super) proof_channel_nonempty: bool,
    pub(super) before_select: u64,
}

#[derive(Default)]
pub(super) struct FinishSnapshot {
    pub(super) state: u64,
    pub(super) hints: u64,
    pub(super) oldest_state_age_ns: u64,
    pub(super) accounts: usize,
    pub(super) storage_accounts: usize,
    pub(super) targets: usize,
    pub(super) proofs_in_flight: usize,
    pub(super) storage_in_flight: usize,
    pub(super) proof_channel_nonempty: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_deferral_is_an_episode_and_hints_do_not_start_one() {
        let mut d = ArrivalDiagnostics::new();
        d.message(MessageKind::Hint, 1, true);
        d.defer(2);
        assert_eq!(d.deferred_skips, 0);
        d.message(MessageKind::State, 3, false);
        d.message(MessageKind::State, 4, true);
        d.defer(5);
        d.defer(7);
        d.flush_reason = Some(FlushReason::InputGap);
        d.flush(11);
        assert_eq!((d.deferred_skips, d.deferred_episodes, d.deferred_ns), (2, 1, 6));
        assert_eq!((d.oldest_state_age_max, d.pending_state, d.pending_hints), (8, 0, 0));
        assert_eq!((d.flushed_state, d.flushed_hints, d.nonempty_selections), (2, 1, [1, 1, 0]));
        d.message(MessageKind::State, 12, false);
        d.message(MessageKind::Finish, 13, false);
        d.finish(FinishSnapshot::default(), 13);
        assert_eq!((d.finish.state, d.finish.hints, d.finish.oldest_state_age_ns), (1, 0, 1));
        d.flush_reason = Some(FlushReason::Finished);
        d.flush(14);
        assert_eq!(d.flushes, [0, 1, 1, 0]);
    }

    #[test]
    fn channel_nonempty_bounds_keep_missing_service_upper_bounds_explicit() {
        let mut d = ArrivalDiagnostics::new();
        d.observe_proofs(1, 2, false);
        d.observe_proofs(4, 5, true);
        let selection = d.observe_proofs(8, 9, true);
        d.proof_service(selection, 12);
        assert_eq!(
            (d.proof_nonempty_lower_ns, d.proof_upper_ns, d.proof_upper_unknown),
            (4, 11, 0)
        );
        d.proofs_drained(12);
        let selection = d.observe_proofs(20, 21, true);
        d.proof_service(selection, 23);
        assert_eq!((d.proof_nonempty_lower_ns, d.proof_upper_ns), (4, 22));
        let selection = d.observe_proofs(30, 31, true);
        d.proof_service(selection, 35);
        assert_eq!(d.proof_upper_unknown, 1);
        // A reservation during a blocking select has no observed nonempty lower bound.
        let selection = d.observe_proofs(40, 41, false);
        d.proof_service(selection, 50);
        assert_eq!((d.proof_nonempty_lower_ns, d.proof_upper_ns), (4, 32));
    }
}
