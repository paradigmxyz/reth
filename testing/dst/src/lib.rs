//! Semantic decisions and strict replay for deterministic system tests.
//!
//! Exploration may use a seed to select among legal options, but a failure artifact records each
//! decision point and concrete choice. Replay rejects the first point whose identity, virtual time,
//! state, legal options, or selected payload changed.

use alloy_primitives::{keccak256, B256};
use bincode::Options as _;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fmt,
    sync::{Arc, Mutex},
};
use thiserror::Error;

/// Current binary trace schema.
pub const TRACE_SCHEMA_VERSION: u16 = 1;

/// Maximum accepted trace size.
pub const MAX_TRACE_BYTES: u64 = 64 * 1024 * 1024;

/// A stable semantic actor name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ActorId(String);

impl ActorId {
    /// Parses a non-empty actor name.
    pub fn new(name: impl Into<String>) -> Result<Self, ActorIdError> {
        let name = name.into();
        if name.is_empty() {
            return Err(ActorIdError);
        }
        Ok(Self(name))
    }

    /// Returns the actor name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ActorId {
    type Error = ActorIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<ActorId> for String {
    fn from(value: ActorId) -> Self {
        value.0
    }
}

impl fmt::Display for ActorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Returned when an actor name is empty.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("actor ID must not be empty")]
pub struct ActorIdError;

/// The kind of semantic choice being made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum DecisionDomain {
    /// Select the next generated workload action.
    Workload = 0,
    /// Order runnable cooperative tasks.
    Schedule = 1,
    /// Advance virtual time or fire a timer.
    Timer = 2,
    /// Deliver an Engine API message.
    EngineDelivery = 3,
    /// Deliver a peer-protocol message.
    PeerDelivery = 4,
    /// Pause, crash, restart, or resume a node.
    NodeLifecycle = 5,
    /// Complete or fail a modeled storage operation.
    Storage = 6,
}

/// Stable identity of one decision point.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DecisionPointId {
    /// Kind of choice.
    pub domain: DecisionDomain,
    /// Actor requesting the choice.
    pub actor: ActorId,
    /// Actor incarnation, incremented after restart.
    pub generation: u32,
    /// Domain-local occurrence for this actor generation.
    pub occurrence: u64,
}

/// Stable identity of an option at a decision point.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OptionId(pub u64);

/// One legal semantic option.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionOption {
    id: OptionId,
    payload: Vec<u8>,
    summary: String,
}

impl DecisionOption {
    /// Creates an option with a stable ID, concrete payload, and diagnostic summary.
    pub fn new(id: OptionId, payload: impl Into<Vec<u8>>, summary: impl Into<String>) -> Self {
        Self { id, payload: payload.into(), summary: summary.into() }
    }

    /// Returns the stable option ID.
    pub const fn id(&self) -> OptionId {
        self.id
    }

    /// Returns the concrete option payload.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Returns the human-readable option summary.
    pub fn summary(&self) -> &str {
        &self.summary
    }
}

/// A parsed decision point with at least one option and no duplicate option IDs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionPoint {
    id: DecisionPointId,
    virtual_time: u64,
    state_digest: B256,
    options_digest: B256,
    options: Vec<DecisionOption>,
}

impl DecisionPoint {
    /// Parses a decision point and rejects empty or ambiguous option sets.
    pub fn new(
        id: DecisionPointId,
        virtual_time: u64,
        state_digest: B256,
        options: Vec<DecisionOption>,
    ) -> Result<Self, DecisionPointError> {
        if options.is_empty() {
            return Err(DecisionPointError::NoOptions);
        }

        let mut ids = BTreeSet::new();
        for option in &options {
            if !ids.insert(option.id) {
                return Err(DecisionPointError::DuplicateOption(option.id));
            }
        }

        let options_digest = digest_options(&options);
        Ok(Self { id, virtual_time, state_digest, options_digest, options })
    }

    /// Returns the semantic point identity.
    pub const fn id(&self) -> &DecisionPointId {
        &self.id
    }

    /// Returns the virtual time at this point.
    pub const fn virtual_time(&self) -> u64 {
        self.virtual_time
    }

    /// Returns the caller-provided digest of relevant model state.
    pub const fn state_digest(&self) -> B256 {
        self.state_digest
    }

    /// Returns a canonical digest of legal option IDs and payloads.
    pub const fn options_digest(&self) -> B256 {
        self.options_digest
    }

    /// Returns the legal options.
    pub fn options(&self) -> &[DecisionOption] {
        &self.options
    }

    fn option(&self, id: OptionId) -> Option<&DecisionOption> {
        self.options.iter().find(|option| option.id == id)
    }
}

fn digest_options(options: &[DecisionOption]) -> B256 {
    let mut canonical = options.iter().collect::<Vec<_>>();
    canonical.sort_unstable_by_key(|option| option.id);

    let mut encoded = Vec::new();
    for option in canonical {
        encoded.extend_from_slice(&option.id.0.to_be_bytes());
        encoded.extend_from_slice(&(option.payload.len() as u64).to_be_bytes());
        encoded.extend_from_slice(&option.payload);
    }
    keccak256(encoded)
}

/// Invalid decision-point options.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DecisionPointError {
    /// No choice can be made.
    #[error("decision point has no legal options")]
    NoOptions,
    /// Two options have the same semantic identity.
    #[error("decision point contains duplicate option {0:?}")]
    DuplicateOption(OptionId),
}

/// Selects one legal option at a semantic decision point.
pub trait DecisionSource {
    /// Returns the selected option ID.
    fn choose(&mut self, point: &DecisionPoint) -> Result<OptionId, ReplayError>;
}

/// Seeded exploration whose choices are keyed by decision identity rather than draw order.
#[derive(Clone, Copy, Debug)]
pub struct Explore {
    seed: u64,
}

impl Explore {
    /// Creates a deterministic explorer.
    pub const fn new(seed: u64) -> Self {
        Self { seed }
    }
}

impl DecisionSource for Explore {
    fn choose(&mut self, point: &DecisionPoint) -> Result<OptionId, ReplayError> {
        let mut encoded = Vec::new();
        encoded.extend_from_slice(&self.seed.to_be_bytes());
        encoded.push(point.id.domain as u8);
        encoded.extend_from_slice(&(point.id.actor.as_str().len() as u64).to_be_bytes());
        encoded.extend_from_slice(point.id.actor.as_str().as_bytes());
        encoded.extend_from_slice(&point.id.generation.to_be_bytes());
        encoded.extend_from_slice(&point.id.occurrence.to_be_bytes());
        encoded.extend_from_slice(&point.virtual_time.to_be_bytes());
        encoded.extend_from_slice(point.state_digest.as_slice());
        encoded.extend_from_slice(point.options_digest.as_slice());
        let digest = keccak256(encoded);
        let mut index = [0u8; 8];
        index.copy_from_slice(&digest[..8]);
        let index = u64::from_be_bytes(index) as usize % point.options.len();
        let mut canonical = point.options.iter().collect::<Vec<_>>();
        canonical.sort_unstable_by_key(|option| option.id);
        Ok(canonical[index].id)
    }
}

/// Metadata required to interpret a trace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceHeader {
    /// Trace schema version.
    pub schema_version: u16,
    /// Source revision used to create the trace.
    pub source_revision: String,
    /// Digest of the chain and simulator configuration.
    pub configuration_digest: B256,
    /// Digest of genesis state.
    pub genesis_digest: B256,
    /// Seed used to explore this case, when the source is seeded.
    pub exploration_seed: Option<u64>,
    /// Number of exploratory decisions requested before stabilization.
    pub decision_budget: Option<u64>,
}

impl TraceHeader {
    /// Creates metadata for the current schema.
    pub fn new(
        source_revision: impl Into<String>,
        configuration_digest: B256,
        genesis_digest: B256,
    ) -> Self {
        Self {
            schema_version: TRACE_SCHEMA_VERSION,
            source_revision: source_revision.into(),
            configuration_digest,
            genesis_digest,
            exploration_seed: None,
            decision_budget: None,
        }
    }

    /// Adds campaign parameters needed to launch the same executor configuration.
    pub const fn with_campaign(mut self, exploration_seed: u64, decision_budget: u64) -> Self {
        self.exploration_seed = Some(exploration_seed);
        self.decision_budget = Some(decision_budget);
        self
    }
}

/// One recorded semantic choice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedDecision {
    /// Semantic identity of the point.
    pub point: DecisionPointId,
    /// Virtual time at the point.
    pub virtual_time: u64,
    /// Digest of the relevant model state.
    pub state_digest: B256,
    /// Digest of every legal option.
    pub options_digest: B256,
    /// Chosen option.
    pub option: OptionId,
    /// Concrete chosen payload.
    pub payload: Vec<u8>,
    /// Human-readable description captured when the failure occurred.
    pub summary: String,
}

/// A complete semantic replay artifact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionTrace {
    /// Trace metadata.
    pub header: TraceHeader,
    /// Choices in execution order.
    pub decisions: Vec<RecordedDecision>,
}

/// Cloneable handle to a trace while a campaign is still running.
///
/// A runner can retain this handle outside the simulator so a panic or timeout does not discard
/// the decisions that led to the failure.
#[derive(Clone, Debug)]
pub struct TraceRecorder(Arc<Mutex<DecisionTrace>>);

impl TraceRecorder {
    /// Starts an empty trace.
    pub fn new(header: TraceHeader) -> Self {
        Self(Arc::new(Mutex::new(DecisionTrace { header, decisions: Vec::new() })))
    }

    /// Returns a point-in-time copy suitable for encoding as a failure artifact.
    pub fn snapshot(&self) -> DecisionTrace {
        self.0.lock().expect("trace recorder lock poisoned").clone()
    }

    fn push(&self, decision: RecordedDecision) {
        self.0.lock().expect("trace recorder lock poisoned").decisions.push(decision);
    }
}

impl DecisionTrace {
    /// Encodes the trace in the bounded binary artifact format.
    pub fn encode(&self) -> Result<Vec<u8>, TraceCodecError> {
        trace_codec().serialize(self).map_err(TraceCodecError::Encode)
    }

    /// Parses a bounded binary trace and rejects unsupported schemas.
    pub fn decode(bytes: &[u8]) -> Result<Self, TraceCodecError> {
        let trace: Self = trace_codec().deserialize(bytes).map_err(TraceCodecError::Decode)?;
        if trace.header.schema_version != TRACE_SCHEMA_VERSION {
            return Err(TraceCodecError::UnsupportedSchema(trace.header.schema_version));
        }
        Ok(trace)
    }
}

fn trace_codec() -> impl bincode::Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .reject_trailing_bytes()
        .with_limit(MAX_TRACE_BYTES)
}

/// Records decisions selected by another source.
#[derive(Debug)]
pub struct Record<S> {
    source: S,
    trace: TraceRecorder,
}

impl<S> Record<S> {
    /// Wraps a decision source and begins a trace.
    pub fn new(source: S, header: TraceHeader) -> Self {
        Self { source, trace: TraceRecorder::new(header) }
    }

    /// Wraps a decision source and writes to a caller-owned recorder.
    pub const fn with_recorder(source: S, trace: TraceRecorder) -> Self {
        Self { source, trace }
    }

    /// Returns a handle that remains readable if the campaign unwinds.
    pub fn recorder(&self) -> TraceRecorder {
        self.trace.clone()
    }

    /// Returns the completed trace.
    pub fn into_trace(self) -> DecisionTrace {
        self.trace.snapshot()
    }
}

impl<S: DecisionSource> DecisionSource for Record<S> {
    fn choose(&mut self, point: &DecisionPoint) -> Result<OptionId, ReplayError> {
        let option_id = self.source.choose(point)?;
        let option = point.option(option_id).ok_or_else(|| {
            ReplayError::SourceSelectedIllegalOption { point: point.id.clone(), option: option_id }
        })?;
        self.trace.push(RecordedDecision {
            point: point.id.clone(),
            virtual_time: point.virtual_time,
            state_digest: point.state_digest,
            options_digest: point.options_digest,
            option: option.id,
            payload: option.payload.clone(),
            summary: option.summary.clone(),
        });
        Ok(option_id)
    }
}

/// Strict trace replayer.
#[derive(Debug)]
pub struct Replay {
    trace: DecisionTrace,
    cursor: usize,
}

impl Replay {
    /// Parses a trace for replay.
    pub fn new(trace: DecisionTrace) -> Result<Self, TraceCodecError> {
        if trace.header.schema_version != TRACE_SCHEMA_VERSION {
            return Err(TraceCodecError::UnsupportedSchema(trace.header.schema_version));
        }
        Ok(Self { trace, cursor: 0 })
    }

    /// Confirms that replay consumed the complete trace.
    pub fn finish(self) -> Result<(), ReplayError> {
        if self.cursor == self.trace.decisions.len() {
            Ok(())
        } else {
            Err(ReplayError::TraceNotFullyConsumed {
                consumed: self.cursor,
                total: self.trace.decisions.len(),
            })
        }
    }
}

impl DecisionSource for Replay {
    fn choose(&mut self, point: &DecisionPoint) -> Result<OptionId, ReplayError> {
        let index = self.cursor;
        let expected = self
            .trace
            .decisions
            .get(index)
            .ok_or_else(|| ReplayError::TraceExhausted { index, actual: point.id.clone() })?;

        if expected.point != point.id {
            return Err(ReplayError::PointChanged {
                index,
                expected: expected.point.clone(),
                actual: point.id.clone(),
            });
        }
        if expected.virtual_time != point.virtual_time {
            return Err(ReplayError::VirtualTimeChanged {
                index,
                expected: expected.virtual_time,
                actual: point.virtual_time,
            });
        }
        if expected.state_digest != point.state_digest {
            return Err(ReplayError::StateChanged {
                index,
                expected: expected.state_digest,
                actual: point.state_digest,
            });
        }
        let actual = point
            .option(expected.option)
            .ok_or(ReplayError::OptionNoLongerLegal { index, option: expected.option })?;
        if actual.payload != expected.payload {
            return Err(ReplayError::PayloadChanged { index, option: expected.option });
        }
        if expected.options_digest != point.options_digest {
            return Err(ReplayError::OptionsChanged {
                index,
                expected: expected.options_digest,
                actual: point.options_digest,
            });
        }

        self.cursor += 1;
        Ok(expected.option)
    }
}

/// Why semantic replay diverged.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ReplayError {
    /// A wrapped source returned an option absent from the point.
    #[error("decision source selected illegal option {option:?} at {point:?}")]
    SourceSelectedIllegalOption {
        /// Decision point.
        point: DecisionPointId,
        /// Illegal option.
        option: OptionId,
    },
    /// Execution requested another decision after the trace ended.
    #[error("trace exhausted at decision {index}; actual point is {actual:?}")]
    TraceExhausted {
        /// Decision index.
        index: usize,
        /// Unexpected point.
        actual: DecisionPointId,
    },
    /// The semantic point identity changed.
    #[error("decision {index} point changed from {expected:?} to {actual:?}")]
    PointChanged {
        /// Decision index.
        index: usize,
        /// Recorded point.
        expected: DecisionPointId,
        /// Current point.
        actual: DecisionPointId,
    },
    /// Virtual time changed.
    #[error("decision {index} virtual time changed from {expected} to {actual}")]
    VirtualTimeChanged {
        /// Decision index.
        index: usize,
        /// Recorded time.
        expected: u64,
        /// Current time.
        actual: u64,
    },
    /// Relevant model state changed.
    #[error("decision {index} state changed from {expected} to {actual}")]
    StateChanged {
        /// Decision index.
        index: usize,
        /// Recorded state digest.
        expected: B256,
        /// Current state digest.
        actual: B256,
    },
    /// The legal option set changed.
    #[error("decision {index} options changed from {expected} to {actual}")]
    OptionsChanged {
        /// Decision index.
        index: usize,
        /// Recorded options digest.
        expected: B256,
        /// Current options digest.
        actual: B256,
    },
    /// The selected option disappeared.
    #[error("decision {index} option {option:?} is no longer legal")]
    OptionNoLongerLegal {
        /// Decision index.
        index: usize,
        /// Recorded option.
        option: OptionId,
    },
    /// The selected option's concrete payload changed.
    #[error("decision {index} option {option:?} changed payload")]
    PayloadChanged {
        /// Decision index.
        index: usize,
        /// Recorded option.
        option: OptionId,
    },
    /// Execution ended before consuming the recorded trace.
    #[error("replay consumed {consumed} of {total} decisions")]
    TraceNotFullyConsumed {
        /// Decisions consumed.
        consumed: usize,
        /// Decisions recorded.
        total: usize,
    },
}

/// Binary trace parsing or encoding failure.
#[derive(Debug, Error)]
pub enum TraceCodecError {
    /// Encoding failed.
    #[error("failed to encode decision trace: {0}")]
    Encode(#[source] bincode::Error),
    /// Decoding failed.
    #[error("failed to decode decision trace: {0}")]
    Decode(#[source] bincode::Error),
    /// The artifact uses an unknown schema.
    #[error("unsupported decision trace schema {0}")]
    UnsupportedSchema(u16),
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Select(OptionId);

    impl DecisionSource for Select {
        fn choose(&mut self, _point: &DecisionPoint) -> Result<OptionId, ReplayError> {
            Ok(self.0)
        }
    }

    fn id(occurrence: u64) -> DecisionPointId {
        DecisionPointId {
            domain: DecisionDomain::Workload,
            actor: ActorId::new("driver").unwrap(),
            generation: 0,
            occurrence,
        }
    }

    fn point(occurrence: u64) -> DecisionPoint {
        DecisionPoint::new(
            id(occurrence),
            42,
            B256::repeat_byte(3),
            vec![
                DecisionOption::new(OptionId(1), b"transfer".to_vec(), "submit transfer"),
                DecisionOption::new(OptionId(2), b"payload".to_vec(), "request payload"),
            ],
        )
        .unwrap()
    }

    fn header() -> TraceHeader {
        TraceHeader::new("revision", B256::repeat_byte(1), B256::repeat_byte(2))
    }

    #[test]
    fn decision_point_parses_invariants() {
        assert_eq!(ActorId::new(""), Err(ActorIdError));
        assert_eq!(
            DecisionPoint::new(id(0), 0, B256::ZERO, Vec::new()),
            Err(DecisionPointError::NoOptions)
        );
        assert_eq!(
            DecisionPoint::new(
                id(0),
                0,
                B256::ZERO,
                vec![
                    DecisionOption::new(OptionId(1), [], "a"),
                    DecisionOption::new(OptionId(1), [], "b"),
                ],
            ),
            Err(DecisionPointError::DuplicateOption(OptionId(1)))
        );
    }

    #[test]
    fn option_digest_is_order_independent() {
        let original = point(0);
        let reversed = DecisionPoint::new(
            id(0),
            42,
            B256::repeat_byte(3),
            original.options.iter().cloned().rev().collect(),
        )
        .unwrap();
        assert_eq!(original.options_digest(), reversed.options_digest());
        assert_eq!(Explore::new(5).choose(&original), Explore::new(5).choose(&reversed));
    }

    #[test]
    fn explore_is_keyed_by_semantic_point() {
        let mut first = Explore::new(7);
        let mut second = Explore::new(7);
        let before = point(999);
        let target = point(2);

        first.choose(&before).unwrap();
        assert_eq!(first.choose(&target).unwrap(), second.choose(&target).unwrap());
    }

    #[test]
    fn records_encodes_and_strictly_replays() {
        let points = [point(0), point(1)];
        let mut recorder = Record::new(Explore::new(9), header());
        let choices =
            points.iter().map(|point| recorder.choose(point).unwrap()).collect::<Vec<_>>();
        let trace = recorder.into_trace();
        let encoded = trace.encode().unwrap();
        let decoded = DecisionTrace::decode(&encoded).unwrap();
        assert_eq!(trace, decoded);

        let mut replay = Replay::new(decoded).unwrap();
        for (point, choice) in points.iter().zip(choices) {
            assert_eq!(replay.choose(point).unwrap(), choice);
        }
        replay.finish().unwrap();
    }

    #[test]
    fn recorder_survives_the_recording_source() {
        let mut recorder = Record::new(Explore::new(9), header());
        let retained = recorder.recorder();
        recorder.choose(&point(0)).unwrap();
        drop(recorder);

        assert_eq!(retained.snapshot().decisions.len(), 1);
    }

    #[test]
    fn replay_reports_first_changed_legal_set() {
        let original = point(0);
        let mut recorder = Record::new(Select(OptionId(2)), header());
        recorder.choose(&original).unwrap();
        let mut replay = Replay::new(recorder.into_trace()).unwrap();
        let changed = DecisionPoint::new(
            id(0),
            42,
            B256::repeat_byte(3),
            vec![DecisionOption::new(OptionId(1), b"transfer".to_vec(), "submit transfer")],
        )
        .unwrap();

        assert!(matches!(
            replay.choose(&changed),
            Err(ReplayError::OptionNoLongerLegal { index: 0, option: OptionId(2) })
        ));
    }

    #[test]
    fn replay_must_consume_the_complete_trace() {
        let mut recorder = Record::new(Explore::new(9), header());
        recorder.choose(&point(0)).unwrap();
        assert_eq!(
            Replay::new(recorder.into_trace()).unwrap().finish(),
            Err(ReplayError::TraceNotFullyConsumed { consumed: 0, total: 1 })
        );
    }

    #[test]
    fn decode_rejects_an_unknown_schema() {
        let mut trace = DecisionTrace { header: header(), decisions: Vec::new() };
        trace.header.schema_version += 1;
        let encoded = trace_codec().serialize(&trace).unwrap();
        assert!(matches!(
            DecisionTrace::decode(&encoded),
            Err(TraceCodecError::UnsupportedSchema(_))
        ));
    }
}
