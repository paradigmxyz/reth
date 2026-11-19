//! Bounded transport and aggregation worker for the experimental EIP-8288 profile.

use alloy_consensus::Transaction;
use alloy_eips::eip2718::Decodable2718;
use alloy_primitives::{bytes::BytesMut, map::B256Set, Bytes, B256};
use futures::{Stream, StreamExt};
use jsonrpsee::{types::ErrorObjectOwned, RpcModule};
use reth_eip8288::{
    prove, transaction_dependencies, DependencyEvidence, DependencyWrapper, ProofError,
    MAX_AGGREGATE_DEPENDENCIES, MAX_PROOF_BYTES, MAX_WRAPPER_BYTES,
};
use reth_eth_wire::{
    capability::SharedCapabilities,
    multiplex::ProtocolConnection,
    protocol::{Protocol, ProtocolIngressLimits},
    Capability,
};
use reth_network::protocol::{ConnectionHandler, OnNotSupported, ProtocolHandler};
use reth_network_api::{Direction, PeerId};
use reth_primitives_traits::SignedTransaction;
use reth_transaction_pool::{PoolConsensusTx, PoolTransaction, TransactionOrigin, TransactionPool};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    future::pending,
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, LazyLock},
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    sync::{mpsc, Semaphore},
    task::JoinHandle,
};
use tracing::debug;

const EVENT_CAPACITY: usize = 16;
const PEER_CAPACITY: usize = 1024;
const OUTBOUND_CAPACITY: usize = 2;
const VALIDATION_CONCURRENCY: usize = 2;
const AGGREGATION_INTERVAL: Duration =
    Duration::from_millis(alloy_eips::eip8288::AGGREGATION_INTERVAL);
const WRAPPER_MESSAGE: u8 = 0;
const RECOVERY_MESSAGE: u8 = 1;
const MAX_MESSAGES_PER_POLL: usize = 32;
static VERIFICATION_PERMITS: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(VALIDATION_CONCURRENCY)));

/// An invalid or incomplete transport package.
#[derive(Debug, thiserror::Error)]
enum AdmissionError {
    #[error(transparent)]
    Proof(#[from] ProofError),
    #[error("wrapper references an unknown transaction")]
    MissingTransaction,
    #[error("invalid frame transaction in dependency wrapper")]
    Transaction,
    #[error("dependency wrapper verification is busy")]
    Busy,
    #[error("dependency transaction rejected: {0}")]
    Pool(#[from] reth_transaction_pool::error::PoolError),
    #[error("dependency verification worker failed: {0}")]
    Worker(#[from] tokio::task::JoinError),
}

// This context is shared by RPC and the peer worker, bounding expensive verification jobs.
#[derive(Clone)]
pub(crate) struct Admission<P> {
    pool: P,
    permits: Arc<Semaphore>,
}

impl<P: TransactionPool + 'static> Admission<P> {
    pub(crate) fn new(pool: P) -> Self {
        Self { pool, permits: VERIFICATION_PERMITS.clone() }
    }

    async fn submit(&self, raw: Bytes) -> Result<Vec<B256>, AdmissionError> {
        if raw.len() > MAX_WRAPPER_BYTES {
            return Err(ProofError::Wrapper.into())
        }
        let permit = self.permits.clone().try_acquire_owned().map_err(|_| AdmissionError::Busy)?;
        let pool = self.pool.clone();
        let span = tracing::Span::current();
        let (transactions, evidence) = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let _entered = span.enter();
            let wrapper = DependencyWrapper::decode(&raw)?;
            let mut transactions = Vec::with_capacity(wrapper.transactions.len());
            let mut hashes = BTreeSet::new();
            for raw in wrapper.transactions {
                let mut transaction = if raw.len() == 32 {
                    pool.get(&B256::from_slice(&raw))
                        .ok_or(AdmissionError::MissingTransaction)?
                        .transaction
                        .clone()
                } else {
                    let mut bytes = raw.as_ref();
                    let transaction = PoolConsensusTx::<P>::decode_2718(&mut bytes)
                        .map_err(|_| AdmissionError::Transaction)?;
                    if !bytes.is_empty() || transaction.frame_transaction().is_none() {
                        return Err(AdmissionError::Transaction)
                    }
                    let recovered = transaction
                        .try_into_recovered()
                        .map_err(|_| AdmissionError::Transaction)?;
                    P::Transaction::try_from_consensus(recovered)
                        .map_err(|_| AdmissionError::Transaction)?
                };
                if transaction.consensus_ref().frame_transaction().is_none() ||
                    !hashes.insert(*transaction.hash())
                {
                    return Err(AdmissionError::Transaction)
                }
                transaction.set_dependency_evidence(wrapper.evidence.clone())?;
                transactions.push(transaction);
            }
            let dependencies = transaction_dependencies(
                transactions.iter().map(|tx| tx.consensus_ref().into_inner()),
            )?;
            if dependencies != wrapper.evidence.dependencies() {
                return Err(ProofError::Coverage.into())
            }
            wrapper.evidence.verify()?;
            Ok::<_, AdmissionError>((transactions, wrapper.evidence))
        })
        .await??;

        // All witnesses and coverage are checked before any transaction is submitted. Pool
        // state can still change between submissions; wrapper admission is not an atomic batch.
        let mut hashes = Vec::with_capacity(transactions.len());
        for transaction in transactions {
            let hash = *transaction.hash();
            if self.pool.get(&hash).is_none() {
                self.pool.add_transaction(TransactionOrigin::External, transaction).await?;
            }
            hashes.push(hash);
        }
        if evidence.wire_witnesses().0 == 1 {
            self.pool.set_dependency_aggregate(Some(evidence));
        }
        Ok(hashes)
    }

    pub(crate) fn rpc(self) -> Result<RpcModule<Self>, jsonrpsee::core::RegisterMethodError> {
        let mut module = RpcModule::new(self);
        module.register_async_method(
            "eth_sendDependencyWrapper",
            |params, context, _| async move {
                let raw: Bytes = params.one()?;
                context
                    .submit(raw)
                    .await
                    .map_err(|error| ErrorObjectOwned::owned(-32000, error.to_string(), None::<()>))
            },
        )?;
        Ok(module)
    }
}

pub(crate) enum Event {
    Opened(PeerId, mpsc::Sender<BytesMut>),
    Message(PeerId, Bytes),
}

#[derive(Clone, Debug)]
pub(crate) struct Handler(mpsc::Sender<Event>);

pub(crate) fn protocol() -> (Handler, mpsc::Receiver<Event>) {
    let (sender, receiver) = mpsc::channel(EVENT_CAPACITY);
    (Handler(sender), receiver)
}

impl ProtocolHandler for Handler {
    type ConnectionHandler = Self;

    fn on_incoming(&self, _: SocketAddr) -> Option<Self> {
        Some(self.clone())
    }

    fn on_outgoing(&self, _: SocketAddr, _: PeerId) -> Option<Self> {
        Some(self.clone())
    }
}

impl ConnectionHandler for Handler {
    type Connection = Connection;

    fn protocol(&self) -> Protocol {
        Protocol::new(Capability::new_static("eip8288", 1), 2)
    }

    fn inbound_limits(&self) -> ProtocolIngressLimits {
        ProtocolIngressLimits::new(MAX_WRAPPER_BYTES + 1)
            .with_max_buffered_bytes((MAX_WRAPPER_BYTES + 1) * OUTBOUND_CAPACITY)
            .with_max_buffered_messages(OUTBOUND_CAPACITY)
    }

    fn on_unsupported_by_peer(
        self,
        _: &SharedCapabilities,
        _: Direction,
        _: PeerId,
    ) -> OnNotSupported {
        OnNotSupported::KeepAlive
    }

    fn into_connection(
        self,
        _: Direction,
        peer: PeerId,
        connection: ProtocolConnection,
    ) -> Connection {
        let (sender, outgoing) = mpsc::channel(OUTBOUND_CAPACITY);
        let admitted = self.0.try_send(Event::Opened(peer, sender)).is_ok();
        Connection { connection, outgoing, events: self.0, peer, admitted }
    }
}

pub(crate) struct Connection {
    connection: ProtocolConnection,
    outgoing: mpsc::Receiver<BytesMut>,
    events: mpsc::Sender<Event>,
    peer: PeerId,
    admitted: bool,
}

impl Stream for Connection {
    type Item = BytesMut;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<BytesMut>> {
        let this = self.get_mut();
        if !this.admitted {
            return Poll::Ready(None)
        }
        if let Poll::Ready(message) = this.outgoing.poll_recv(cx) {
            return Poll::Ready(message)
        }
        for _ in 0..MAX_MESSAGES_PER_POLL {
            match this.connection.poll_next_unpin(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Ready(Some(message)) => {
                    if message.is_empty() ||
                        message.len() > MAX_WRAPPER_BYTES + 1 ||
                        message[0] > RECOVERY_MESSAGE ||
                        (message[0] == RECOVERY_MESSAGE && message.len() != 1)
                    {
                        return Poll::Ready(None)
                    }
                    // Backpressure bounds memory. A dropped hash wrapper can be recovered from
                    // the next aggregate snapshot or an explicit recovery request.
                    match this.events.try_send(Event::Message(this.peer, message.freeze().into())) {
                        Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
                        Err(mpsc::error::TrySendError::Closed(_)) => return Poll::Ready(None),
                    }
                }
            }
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

struct Peer {
    outgoing: mpsc::Sender<BytesMut>,
    known: Arc<B256Set>,
    needs_full: bool,
}

struct Snapshot {
    transactions: Vec<(B256, Bytes)>,
    hashes: Arc<B256Set>,
    evidence: Arc<DependencyEvidence>,
}

fn announce(peer: &mut Peer, snapshot: &Snapshot) {
    let transactions = snapshot
        .transactions
        .iter()
        .map(|(hash, raw)| {
            if !peer.needs_full && peer.known.contains(hash) {
                Bytes::copy_from_slice(hash.as_slice())
            } else {
                raw.clone()
            }
        })
        .collect();
    let wrapper = DependencyWrapper { transactions, evidence: snapshot.evidence.clone() };
    let Ok(raw) = wrapper.encode() else { return };
    let mut message = BytesMut::with_capacity(raw.len() + 1);
    message.extend_from_slice(&[WRAPPER_MESSAGE]);
    message.extend_from_slice(&raw);
    if peer.outgoing.try_send(message).is_ok() {
        peer.known = Arc::clone(&snapshot.hashes);
        peer.needs_full = false;
    }
}

/// Runs one prover job at a time. Snapshot sizes and all peer queues are bounded.
pub(crate) async fn run<P: TransactionPool + 'static>(
    admission: Admission<P>,
    mut events: mpsc::Receiver<Event>,
) {
    let mut peers = BTreeMap::<PeerId, Peer>::new();
    let mut snapshot: Option<Snapshot> = None;
    let mut job: Option<JoinHandle<Result<Snapshot, ProofError>>> = None;
    let mut interval = tokio::time::interval(AGGREGATION_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            event = events.recv() => match event {
                None => return,
                Some(Event::Opened(id, outgoing)) => {
                    if peers.len() < PEER_CAPACITY {
                        peers.insert(id, Peer { outgoing, known: Arc::default(), needs_full: true });
                    }
                }
                Some(Event::Message(id, message)) => {
                    if message[0] == RECOVERY_MESSAGE {
                        if let Some(peer) = peers.get_mut(&id) { peer.needs_full = true; }
                    } else if let Err(error) = admission.submit(message.slice(1..)).await {
                        if matches!(error, AdmissionError::MissingTransaction) &&
                            let Some(peer) = peers.get_mut(&id) &&
                            let Err(error) = peer.outgoing.try_send(BytesMut::from(&[RECOVERY_MESSAGE][..]))
                        {
                            debug!(target: "reth::eip8288", %error, "Recovery queue unavailable");
                        }
                        debug!(target: "reth::eip8288", %error, "Rejected dependency wrapper");
                    }
                }
            },
            result = async { match &mut job { Some(job) => job.await, None => pending().await } } => {
                job = None;
                match result {
                    Ok(Ok(next)) if {
                        let current: B256Set = admission.pool.pending_transactions_max(MAX_AGGREGATE_DEPENDENCIES)
                            .iter().filter(|tx| tx.propagate).map(|tx| *tx.transaction.hash()).collect();
                        next.transactions.iter().all(|(hash, _)| current.contains(hash))
                    } => {
                        for peer in peers.values_mut() { announce(peer, &next); }
                        admission.pool.set_dependency_aggregate(Some(next.evidence.clone()));
                        snapshot = Some(next);
                    }
                    Ok(Err(error)) => debug!(target: "reth::eip8288", %error, "Aggregate snapshot failed"),
                    Err(error) => debug!(target: "reth::eip8288", %error, "Aggregate worker failed"),
                    _ => {}
                }
            },
            _ = interval.tick() => {
                peers.retain(|_, peer| !peer.outgoing.is_closed());
                if peers.is_empty() { continue }
                let mut pending = admission.pool.pending_transactions_max(MAX_AGGREGATE_DEPENDENCIES);
                let current: B256Set = pending.iter().filter(|tx| tx.propagate).map(|tx| *tx.transaction.hash()).collect();
                if let Some(snapshot) = &snapshot &&
                    snapshot.transactions.iter().all(|(hash, _)| current.contains(hash))
                {
                    for peer in peers.values_mut() { announce(peer, snapshot); }
                }
                if job.is_some() { continue }
                pending.sort_unstable_by_key(|tx| *tx.transaction.hash());
                let mut transactions = Vec::new();
                let mut inputs = Vec::new();
                let mut input_ids = HashSet::new();
                let mut dependencies = BTreeSet::new();
                let mut bytes = 0;
                for tx in pending {
                    if !tx.propagate { continue }
                    let consensus = tx.transaction.consensus_ref();
                    if let Some(evidence) = tx.transaction.dependency_evidence() &&
                        let Some(frame) = consensus.frame_transaction()
                    {
                        let deps = match frame.dependencies() {
                            Ok(deps) => deps,
                            Err(error) => {
                                debug!(target: "reth::eip8288", %error, "Cannot reconstruct pending dependencies");
                                continue;
                            }
                        };
                        let raw = tx.transaction.encoded_2718_consensus();
                        // Reserve room for the proof, dependency list, and RLP framing.
                        let next_bytes = bytes + raw.len() + deps.len() * 100 + 16;
                        if next_bytes > MAX_WRAPPER_BYTES - MAX_PROOF_BYTES ||
                            dependencies.len() + deps.len() > MAX_AGGREGATE_DEPENDENCIES { break }
                        bytes = next_bytes;
                        dependencies.extend(deps);
                        transactions.push((*tx.transaction.hash(), raw));
                        if input_ids.insert(Arc::as_ptr(evidence) as usize) { inputs.push(evidence.clone()); }
                    }
                }
                if transactions.is_empty() {
                    snapshot = None;
                    admission.pool.set_dependency_aggregate(None);
                    continue
                }
                if snapshot.as_ref().is_some_and(|last| last.transactions == transactions) { continue }
                // A previous aggregate can replace old witnesses; OutputClaims::Only drops
                // dependencies belonging exclusively to removed transactions.
                if let Some(previous) = &snapshot {
                    inputs.retain(|input| input.dependencies().iter().any(|dep|
                        previous.evidence.dependencies().binary_search(dep).is_err()));
                    inputs.push(previous.evidence.clone());
                }
                let dependencies: Vec<_> = dependencies.into_iter().collect();
                let span = tracing::Span::current();
                job = Some(tokio::task::spawn_blocking(move || {
                    let _entered = span.enter();
                    let proof = prove(&dependencies, &inputs)?;
                    let evidence = Arc::new(DependencyEvidence::aggregate(dependencies, proof.stark_proof)?);
                    let hashes = Arc::new(transactions.iter().map(|(hash, _)| *hash).collect());
                    Ok(Snapshot { transactions, hashes, evidence })
                }));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announcements_recover_full_transactions_and_respect_backpressure() {
        let hash = B256::repeat_byte(1);
        let raw = Bytes::from(vec![2; 33]);
        let snapshot = Snapshot {
            transactions: vec![(hash, raw.clone())],
            hashes: Arc::new([hash].into_iter().collect()),
            evidence: Arc::new(DependencyEvidence::aggregate(vec![], Bytes::new()).unwrap()),
        };
        let (outgoing, mut incoming) = mpsc::channel(1);
        outgoing.try_send(BytesMut::new()).unwrap();
        let mut peer = Peer { outgoing, known: Arc::default(), needs_full: true };
        announce(&mut peer, &snapshot);
        assert!(peer.known.is_empty());
        assert!(peer.needs_full);
        incoming.try_recv().unwrap();
        announce(&mut peer, &snapshot);
        let full = incoming.try_recv().unwrap();
        assert_eq!(full[0], WRAPPER_MESSAGE);
        assert_eq!(DependencyWrapper::decode(&full[1..]).unwrap().transactions, vec![raw.clone()]);
        assert!(Arc::ptr_eq(&peer.known, &snapshot.hashes));
        announce(&mut peer, &snapshot);
        let hashes = incoming.try_recv().unwrap();
        assert_eq!(
            DependencyWrapper::decode(&hashes[1..]).unwrap().transactions,
            vec![Bytes::copy_from_slice(hash.as_slice())]
        );
        peer.needs_full = true;
        announce(&mut peer, &snapshot);
        let recovered = incoming.try_recv().unwrap();
        assert_eq!(DependencyWrapper::decode(&recovered[1..]).unwrap().transactions, vec![raw]);
    }
}
