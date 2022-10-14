use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{
        mpsc::{self, UnboundedReceiver, UnboundedSender},
        RwLock,
    },
    task, time,
};
use crate::{
    alert::{Alert, AlertSender},
    conf::TorrentConf,
    counter::ThruputCounters,
    disk::{
        self,
        error::{ReadError, WriteError},
    },
    download::PieceDownload,
    error::Error,
    peer::{self, ConnectionState, PeerSession, SessionState, SessionTick},
    piece_picker::PiecePicker,
    storage_info::StorageInfo,
    tracker::{Announce, Event, Tracker},
    Bitfield, BlockInfo, PeerId, PieceIndex, Sha1Hash, TorrentId,
};
use error::*;
use stats::{Peers, PieceStats, ThruputStats, TorrentStats};
use crate::info::{PieceIndex, StorageInfo};
use crate::sha1::{PeerId, Sha1Hash};

pub mod error;
pub mod stats;

/// The channel for communicating with torrent.
pub(crate) type Sender = UnboundedSender<Command>;

/// The type of channel on which a torrent can listen for block write
/// completions.
pub(crate) type Receiver = UnboundedReceiver<Command>;

/// The types of messages that the torrent can receive from other parts of the
/// engine.
#[derive(Debug)]
pub(crate) enum Command {
    /// Sent when some blocks were written to disk or an error ocurred while
    /// writing.
    PieceCompletion(Result<PieceCompletion, WriteError>),
    /// There was an error reading a block.
    ReadError {
        block_info: BlockInfo,
        error: ReadError,
    },
    /// A message sent only once, after the peer has been connected.
    PeerConnected { addr: SocketAddr, id: PeerId },
    /// Peer sessions periodically send this message when they have a state
    /// change.
    PeerState { addr: SocketAddr, info: SessionTick },
    /// Gracefully shut down the torrent.
    ///
    /// This command tells all active peer sessions of torrent to do the same,
    /// waits for them and announces to trackers our exit.
    Shutdown,
}

/// The type returned on completing a piece.
#[derive(Debug)]
pub(crate) struct PieceCompletion {
    /// The index of the piece.
    pub index: PieceIndex,
    /// Whether the piece is valid. If it's not, it's not written to disk.
    pub is_valid: bool,
}

/// Information and methods shared with peer sessions in the torrent.
///
/// This type contains fields that need to be read or updated by peer sessions.
/// Fields expected to be mutated are thus secured for inter-task access with
/// various synchronization primitives.
pub(crate) struct TorrentContext {
    /// The torrent ID, unique in this engine.
    pub id: TorrentId,
    /// The info hash of the torrent, derived from its metainfo. This is used to
    /// identify the torrent with other peers and trackers.
    pub info_hash: Sha1Hash,
    /// The arbitrary client id, chosen by the user of this library. This is
    /// advertised to peers and trackers.
    pub client_id: PeerId,

    /// A copy of the torrent channel sender. This is not used by torrent iself,
    /// but by the peer session tasks to which an arc copy of this torrent
    /// context is given.
    pub cmd_tx: Sender,

    /// The piece picker picks the next most optimal piece to download and is
    /// shared by all peers in a torrent.
    pub piece_picker: Arc<RwLock<PiecePicker>>,
    /// These are the active piece downloads in which the peer sessions in this
    /// torrent are participating.
    ///
    /// They are stored and synchronized in this object to download a piece from
    /// multiple peers, which helps us to have fewer incomplete pieces.
    ///
    /// Peer sessions may be run on different threads, any of which may read and
    /// write to this map and to the pieces in the map. Thus we need a read
    /// write lock on both.
    // TODO: Benchmark whether using the nested locking approach isn't too slow.
    // For mvp it should do.
    pub downloads: RwLock<HashMap<PieceIndex, RwLock<PieceDownload>>>,

    /// The channel on which to post alerts to user.
    pub alert_tx: AlertSender,

    /// The handle to the disk IO task, used to issue commands on it. A copy of
    /// this handle is passed down to each peer session.
    pub disk_tx: disk::Sender,
    /// Info about the torrent's storage (piece length, download length, etc).
    pub storage: StorageInfo,
}

/// Parameters for the torrent constructor.
pub(crate) struct Params {
    pub id: TorrentId,
    pub disk_tx: disk::Sender,
    pub info_hash: Sha1Hash,
    pub storage_info: StorageInfo,
    pub own_pieces: Bitfield,
    pub trackers: Vec<Tracker>,
    pub client_id: PeerId,
    pub listen_addr: SocketAddr,
    pub conf: TorrentConf,
    pub alert_tx: AlertSender,
}

/// Represents a torrent upload or download.
///
/// This is the main entity responsible for the high-level management of
/// a torrent download or upload. It starts and stops connections with peers
/// ([`PeerSession`](crate::peer::PeerSession) instances) and stores metadata
/// about the torrent.
pub(crate) struct Torrent {
    /// The peers in this torrent.
    peers: HashMap<SocketAddr, PeerSessionEntry>,
    /// The peers returned by tracker to which we can connect.
    available_peers: Vec<SocketAddr>,
    /// Information that is shared with peer sessions.
    ctx: Arc<TorrentContext>,
    /// The port on which other entities in the engine send this torrent
    /// messages.
    ///
    /// The channel has to be wrapped in a `stream::Fuse` so that we can
    /// `select!` on it in the torrent event loop.
    cmd_rx: Receiver,
    /// The trackers we can announce to.
    trackers: Vec<TrackerEntry>,

    /// The address on which torrent should listen for new peers.
    listen_addr: SocketAddr,

    /// The time the torrent was first started.
    start_time: Option<Instant>,
    /// The total time the torrent has been running.
    ///
    /// This is a separate field as `Instant::now() - start_time` cannot be
    /// relied upon due to the fact that it is possible to pause a torrent, in
    /// which case we don't want to record the run time.
    // TODO: pausing a torrent is not actually at this point, but this is done
    // in expectation of that feature
    run_duration: Duration,

    /// In the last part of the download the torrent is in what's called the
    /// endgame. This is the stage when all pieces have been picked but not all
    /// have been received. There is a tendency for a piece to be mostly
    /// downloaded by one peer, but when only a few pieces are left to complete
    /// the torrent this could defer completion because some of these last
    /// pieces may end up with slower peers.  So when endgame is active, we let
    /// all peers finish the remaining pieces and cancel pending requests from
    /// the slower peers.
    in_endgame: bool,

    /// Measures various transfer statistics.
    counters: ThruputCounters,

    /// The configuration of this particular torrent.
    conf: TorrentConf,

    /// If `TorrentAlertConf::latest_completed_pieces` alert type is set, each
    /// round the torrent collects the pieces that were downloaded, sends them
    /// to peer as an alert, and resets the list.
    ///
    /// This is set to some if the configuration is enabled, and set to none if
    /// disabled.
    completed_pieces: Option<Vec<PieceIndex>>,
}

impl Torrent {
    /// Creates a new `Torrent` instance for downloading or seeding a torrent.
    ///
    /// # Important
    ///
    /// This constructor only initializes the torrent components but does not
    /// actually start it. See [`Self::start`].
    pub fn new(params: Params) -> (Self, Sender) {
        let Params {
            id,
            disk_tx,
            info_hash,
            storage_info,
            own_pieces,
            trackers,
            client_id,
            listen_addr,
            conf,
            alert_tx,
        } = params;

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let piece_picker = PiecePicker::new(own_pieces);
        let trackers = trackers.into_iter().map(TrackerEntry::new).collect();
        let completed_pieces = if conf.alerts.completed_pieces {
            Some(Vec::new())
        } else {
            None
        };

        (
            Self {
                peers: HashMap::new(),
                available_peers: Vec::new(),
                ctx: Arc::new(TorrentContext {
                    id,
                    cmd_tx: cmd_tx.clone(),
                    piece_picker: Arc::new(RwLock::new(piece_picker)),
                    downloads: RwLock::new(HashMap::new()),
                    info_hash,
                    client_id,
                    alert_tx,
                    disk_tx,
                    storage: storage_info,
                }),
                start_time: None,
                run_duration: Duration::default(),
                cmd_rx,
                trackers,
                in_endgame: false,
                counters: Default::default(),
                listen_addr,
                conf,
                completed_pieces,
            },
            cmd_tx,
        )
    }

    /// Starts the torrent and runs until an error is encountered.
    pub async fn start(&mut self, peers: &[SocketAddr]) -> Result<()> {
        log::info!("Starting torrent");

        self.available_peers.extend_from_slice(peers);

        // record the torrent starttime
        self.start_time = Some(Instant::now());

        // if the torrent is a seed, don't send the started event, just an
        // empty announce
        let tracker_event =
            if self.ctx.piece_picker.read().await.missing_piece_count() == 0 {
                None
            } else {
                Some(Event::Started)
            };
        if let Err(e) = self
            .announce_to_trackers(Instant::now(), tracker_event)
            .await
        {
            // this is a torrent error, not a tracker error, as that is handled
            // inside the function
            self.ctx
                .alert_tx
                .send(Alert::Error(Error::Torrent {
                    id: self.ctx.id,
                    error: e,
                }))
                .ok();
        }

        if let Err(e) = self.run().await {
            // send alert of torrent failure to user
            self.ctx
                .alert_tx
                .send(Alert::Error(Error::Torrent {
                    id: self.ctx.id,
                    error: e,
                }))
                .ok();
        }

        Ok(())
    }

    /// Starts the torrent and runs until an error is encountered.
    async fn run(&mut self) -> Result<()> {
        let mut tick_timer = time::interval(Duration::from_secs(1));
        let mut last_tick_time = None;

        let listener = TcpListener::bind(&self.listen_addr).await?;
        // the bind port may have been 0, so we need to get the actual port in
        // use
        self.listen_addr = listener.local_addr()?;

        // the torrent loop is triggered every second by the loop timer and by
        // disk IO events
        loop {
            tokio::select! {
                tick_time = tick_timer.tick() => {
                    self.tick(&mut last_tick_time, tick_time.into_std()).await?;
                }
                peer_conn_result = listener.accept() => {
                    let (socket, addr) = match peer_conn_result {
                        Ok((socket, addr)) => (socket, addr),
                        Err(e) => {
                            log::info!("Error accepting peer connection: {}", e);
                            continue;
                        }
                    };
                    log::info!("New connection {:?}", addr);

                    // start inbound session
                    let (session, tx) = PeerSession::new(
                        Arc::clone(&self.ctx),
                        addr,
                    );
                    self.peers.insert(addr, PeerSessionEntry::start_inbound(socket, session, tx));
                }
                Some(cmd) = self.cmd_rx.recv() => {
                    match cmd {
                        Command::PeerConnected { addr, id } => {
                            if let Some(peer) = self.peers.get_mut(&addr) {
                                log::debug!(
                                    "Peer {} connected with client '{}', \
                                    updating state",
                                    addr, String::from_utf8_lossy(&id)
                                );
                                peer.id = Some(id);
                            }
                        }
                        Command::PeerState { addr, info } => {
                            self.handle_peer_state_change(addr, info);
                        }
                        Command::PieceCompletion(write_result) => {
                            log::debug!("Disk write result {:?}", write_result);
                            match write_result {
                                Ok(piece) => {
                                    self.handle_piece_completion(piece).await?;
                                }
                                Err(e) => {
                                    log::error!(
                                        "Failed to write piece to disk: {}",
                                        e
                                    );
                                }
                            }
                        }
                        Command::ReadError { block_info, error } => {
                            log::error!(
                                "Failed to read from disk {}: {}",
                                block_info,
                                error
                            );
                            // TODO: For now we just log for simplicity's sake, but in the
                            // future we'll need error recovery mechanisms here.
                            // For instance, it may be that the torrent file got moved while
                            // the torrent was still seeding. In this case we'd need to stop
                            // torrent and send an alert to the API consumer.
                        }
                        Command::Shutdown => {
                            self.shutdown().await?;
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// The torrent tick, as in "the tick of a clock", which runs every second
    /// to perform periodic updates.
    ///
    /// This is when we update statistics and report them to the user, when new
    /// peers are connected, and when perioric announces are made.
    async fn tick(
        &mut self,
        last_tick_time: &mut Option<Instant>,
        now: Instant,
    ) -> Result<()> {
        // calculate how long torrent has been running

        let elapsed_since_last_tick = last_tick_time
            .or(self.start_time)
            .map(|t| now.saturating_duration_since(t))
            .unwrap_or_default();
        self.run_duration += elapsed_since_last_tick;
        *last_tick_time = Some(now);

        // check if we can connect some peers
        // NOTE: do this before announcing as we don't want to block new
        // connections with the potentially long running announce requests
        self.connect_peers();

        // check if we need to announce to some trackers
        let event = None;
        self.announce_to_trackers(now, event).await?;

        log::debug!(
            "Stats: \
            elapsed {} s, \
            download: {} b/s (peak: {} b/s, total: {} b) wasted: {} b \
            upload: {} b/s (peak: {} b/s, total: {} b)",
            self.run_duration.as_secs(),
            self.counters.payload.down.avg(),
            self.counters.payload.down.peak(),
            self.counters.payload.down.total(),
            self.counters.waste.total(),
            self.counters.payload.up.avg(),
            self.counters.payload.up.peak(),
            self.counters.payload.up.total(),
        );

        // TODO: consider removing this check, it's expensive, or caching it in
        // piece picker
        if log::log_enabled!(log::Level::Debug) {
            let piece_picker_guard = self.ctx.piece_picker.read().await;
            let unavailable_piece_count =
                piece_picker_guard.pieces().iter().fold(0, |acc, piece| {
                    if piece.frequency == 0 {
                        acc + 1
                    } else {
                        acc
                    }
                });
            if unavailable_piece_count > 0 {
                log::debug!(
                    "Torrent swarm doesn't have all pieces (missing: {})",
                    unavailable_piece_count
                );
            }
        }

        // send periodic stats update to api user
        let stats = self.build_stats().await;
        self.ctx
            .alert_tx
            .send(Alert::TorrentStats {
                id: self.ctx.id,
                stats: Box::new(stats),
            })
            .ok();

        self.counters.reset();

        Ok(())
    }

    /// Attempts to connect available peers, if we have any.
    fn connect_peers(&mut self) {
        let connect_count = self
            .conf
            .max_connected_peer_count
            .saturating_sub(self.peers.len())
            .min(self.available_peers.len());
        if connect_count == 0 {
            log::trace!("Cannot connect to peers");
            return;
        }

        log::debug!("Connecting {} peer(s)", connect_count);
        for addr in self.available_peers.drain(0..connect_count) {
            log::info!("Connecting to peer {}", addr);
            let (session, tx) = PeerSession::new(Arc::clone(&self.ctx), addr);
            self.peers
                .insert(addr, PeerSessionEntry::start_outbound(session, tx));
        }
    }

    /// Chacks whether we need to announce to any trackers of if we need to request
    /// peers.
    async fn announce_to_trackers(
        &mut self,
        now: Instant,
        event: Option<Event>,
    ) -> Result<()> {
        // calculate transfer statistics in advance
        let uploaded = self.counters.payload.up.total();
        let downloaded = self.counters.payload.down.total();
        let left = self.ctx.storage.download_len - downloaded;

        // skip trackers that errored too often
        // TODO: introduce a retry timeout
        let tracker_error_threshold = self.conf.tracker_error_threshold;
        for tracker in self
            .trackers
            .iter_mut()
            .filter(|t| t.error_count < tracker_error_threshold)
        {
            // Check if the torrent's peer count has fallen below the minimum.
            // But don't request new peers otherwise or if we're about to stop
            // torrent.
            let peer_count = self.peers.len() + self.available_peers.len();
            let needed_peer_count = if peer_count
                >= self.conf.min_requested_peer_count
                || event == Some(Event::Stopped)
            {
                None
            } else {
                debug_assert!(self.conf.max_connected_peer_count >= peer_count);
                let needed = self.conf.max_connected_peer_count - peer_count;
                // Download at least this numbe of peers, even if we don't need
                // as many. This is because later we may be able to connect to
                // more peers and in that case we don't want to wait till the
                // next tracker request.
                Some(self.conf.min_requested_peer_count.max(needed))
            };

            // we can override the normal annoucne interval if we need peers or
            // if we have an event to announce
            if event.is_some()
                || (needed_peer_count > Some(0)
                && tracker.can_announce(now, self.conf.announce_interval))
                || tracker.should_announce(now, self.conf.announce_interval)
            {
                let params = Announce {
                    tracker_id: tracker.id.clone(),
                    info_hash: self.ctx.info_hash,
                    peer_id: self.ctx.client_id,
                    port: self.listen_addr.port(),
                    peer_count: needed_peer_count,
                    uploaded,
                    downloaded,
                    left,
                    ip: None,
                    event,
                };
                // TODO: We probably don't want to block the torrent event loop
                // here waiting on the tracker response. Instead, poll the
                // future in the event loop select call, or spawn the tracker
                // announce on a separate task and return the result as
                // an mpsc message.
                match tracker.client.announce(params).await {
                    Ok(resp) => {
                        log::info!(
                            "Announced to tracker {}, response: {:?}",
                            tracker.client,
                            resp
                        );
                        if let Some(tracker_id) = resp.tracker_id {
                            tracker.id = Some(tracker_id);
                        }
                        if let Some(failure_reason) = resp.failure_reason {
                            log::warn!(
                                "Error contacting tracker {}: {}",
                                tracker.client,
                                failure_reason
                            );
                        }
                        if let Some(warning_message) = resp.warning_message {
                            log::warn!(
                                "Warning from tracker {}: {}",
                                tracker.client,
                                warning_message
                            );
                        }
                        if let Some(interval) = resp.interval {
                            log::info!(
                                "Tracker {} interval: {} s",
                                tracker.client,
                                interval.as_secs()
                            );
                            tracker.interval = Some(interval);
                        }
                        if let Some(min_interval) = resp.min_interval {
                            log::info!(
                                "Tracker {} min min_interval: {} s",
                                tracker.client,
                                min_interval.as_secs()
                            );
                            tracker.min_interval = Some(min_interval);
                        }

                        if let (Some(seeder_count), Some(leecher_count)) =
                        (resp.seeder_count, resp.leecher_count)
                        {
                            log::debug!(
                                "Torrent seeds: {} and leeches: {}",
                                seeder_count,
                                leecher_count
                            );
                        }

                        if !resp.peers.is_empty() {
                            log::debug!(
                                "Received peers from tracker {}: {:?}",
                                tracker.client,
                                resp.peers
                            );
                            self.available_peers.extend(resp.peers.into_iter());
                        }
                    }
                    Err(e) => {
                        log::warn!(
                            "Error announcing to tracker {}: {}",
                            tracker.client,
                            e
                        );
                        tracker.error_count += 1;
                        self.ctx.alert_tx.send(Alert::Error(
                            Error::Tracker {
                                id: self.ctx.id,
                                error: e,
                            },
                        ))?;
                    }
                }
                tracker.last_announce_time = Some(now);
            }
        }

        Ok(())
    }

    /// Returns high-level statistics about the torrent for sending to the user.
    async fn build_stats(&mut self) -> TorrentStats {
        let missing_piece_count =
            self.ctx.piece_picker.read().await.missing_piece_count();
        let piece_count = self.ctx.storage.piece_count;
        let completed_pieces =
            self.completed_pieces.as_mut().map(std::mem::take);
        let peers = if self.conf.alerts.peers {
            let peers = self
                .peers
                .iter()
                .map(|(addr, entry)| stats::PeerSessionStats {
                    addr: *addr,
                    id: entry.id,
                    state: entry.state,
                    piece_count: entry.piece_count,
                    thruput: entry.thruput,
                })
                .collect();
            Peers::Full(peers)
        } else {
            Peers::Count(self.peers.len())
        };

        TorrentStats {
            start_time: self.start_time,
            run_duration: self.run_duration,
            pieces: PieceStats {
                total: piece_count,
                complete: piece_count - missing_piece_count,
                pending: self.ctx.downloads.read().await.len(),
                latest_completed: completed_pieces,
            },
            thruput: ThruputStats::from(&self.counters),
            peers,
        }
    }

    /// Handles the message that peer sessions send to torrent when their state
    /// changed.
    ///
    /// It simply updates the minimum copy of the peer's state that is kept in
    /// torrent in order to perform various pieces of logic (the choke
    /// algorithm and detailed reporting to user, neither of which is done at
    /// the moment).
    fn handle_peer_state_change(
        &mut self,
        addr: SocketAddr,
        info: SessionTick,
    ) {
        if let Some(peer) = self.peers.get_mut(&addr) {
            log::debug!("Updating peer {} state", addr);

            peer.state = info.state;
            peer.piece_count = info.piece_count;
            peer.thruput = ThruputStats::from(&info.counters);

            // update torrent thruput stats
            self.counters += &info.counters;

            // if we disconnected peer, remove it
            if peer.state.connection == ConnectionState::Disconnected {
                self.peers.remove(&addr);
            }
        } else {
            log::debug!("Tried updating non-existent peer {}", addr);
        }
    }

    /// Does some bookkeeping to mark the piece as finished. All peer sessions
    /// are notified of the newly downloaded piece.
    async fn handle_piece_completion(
        &mut self,
        piece: PieceCompletion,
    ) -> Result<()> {
        // if this write completed a piece, check torrent
        // completion
        if piece.is_valid {
            // remove download entry
            self.ctx.downloads.write().await.remove(&piece.index);

            // register piece in piece picker
            let mut piece_picker_write_guard =
                self.ctx.piece_picker.write().await;

            piece_picker_write_guard.received_piece(piece.index);
            let missing_piece_count =
                piece_picker_write_guard.missing_piece_count();

            // Even if we don't have all pieces, they may all have already
            // been picked. In this case we need to enter endgame mode, if not
            // already in it.
            if !self.in_endgame
                && missing_piece_count > 0
                && piece_picker_write_guard.all_pieces_picked()
            {
                log::info!("Torrent entering endgame");
                self.in_endgame = true;
            }

            // we don't need the lock anymore
            drop(piece_picker_write_guard);

            log::info!(
                "Downloaded piece {} (left: {})",
                piece.index,
                missing_piece_count
            );

            if let Some(latest_completed_pieces) = &mut self.completed_pieces {
                latest_completed_pieces.push(piece.index);
            }

            // tell all sessions that we got a new piece so that they can send
            // a "have(piece)" message to their peers or cancel potential
            // duplicate requests for the same piece
            for peer in self.peers.values() {
                if let Some(tx) = &peer.tx {
                    // this may be after the peer session had already stopped
                    // but before the torrent tick ran and got a chance to reap
                    // the dead session
                    tx.send(peer::Command::PieceCompletion {
                        index: piece.index,
                        in_endgame: self.in_endgame,
                    })
                        .ok();
                }
            }

            // if the torrent is fully downloaded, stop the download loop
            if missing_piece_count == 0 {
                log::info!(
                    "Finished torrent download, exiting. \
                    Peak download rate: {} b/s, wasted: {} b",
                    self.counters.payload.down.peak(),
                    self.counters.waste.total(),
                );

                // notify user of torrent completion
                self.ctx
                    .alert_tx
                    .send(Alert::TorrentComplete(self.ctx.id))
                    .ok();

                // tell trackers we've finished
                self.announce_to_trackers(
                    Instant::now(),
                    Some(Event::Completed),
                )
                    .await?;
            }
        } else {
            // TODO(https://github.com/mandreyel/cratetorrent/issues/61):
            // implement parole mode for the peers that sent corrupt data
            log::warn!("Piece {} is invalid", piece.index);
            // mark all blocks free to be requested in piece
            if let Some(piece) =
            self.ctx.downloads.read().await.get(&piece.index)
            {
                piece.write().await.free_all_blocks();
            }
        }

        Ok(())
    }

    /// Shuts down torrent and all peer sessions, and also announces torrent's
    /// exit to tracker.
    async fn shutdown(&mut self) -> Result<()> {
        // send shutdown command to all connected peers
        for peer in self.peers.values() {
            if let Some(tx) = &peer.tx {
                // we don't particularly care if we weren't successful
                // in sending the command (for now)
                tx.send(peer::Command::Shutdown).ok();
            }
        }

        for peer in self.peers.values_mut() {
            if let Err(e) = peer
                .join_handle
                .take()
                .expect("peer join handle missing")
                .await
                .expect("task error")
            {
                log::error!("Peer session error: {}", e);
            }
        }

        // tell trackers we're leaving
        self.announce_to_trackers(Instant::now(), Some(Event::Stopped))
            .await
    }
}

/// A peer in the torrent. Contains additional metadata needed by torrent to
/// manage the peer.
struct PeerSessionEntry {
    /// The channel on which to communicate with the peer session.
    ///
    /// This is set when the session is started.
    tx: Option<peer::Sender>,

    /// Peer's 20 byte BitTorrent id. Updated when the peer sends us its peer
    /// id, in the handshake.
    id: Option<PeerId>,
    /// Cached information about the session state. Updated every time peer
    /// updates us.
    state: SessionState,
    /// The number of pieces that the peer has available.
    piece_count: usize,

    /// Most recent throughput statistics of this peer.
    thruput: ThruputStats,

    /// The peer session task's join handle, used during shutdown.
    join_handle: Option<task::JoinHandle<peer::error::Result<()>>>,
}

impl PeerSessionEntry {
    fn start_outbound(mut session: PeerSession, tx: peer::Sender) -> Self {
        let join_handle =
            task::spawn(async move { session.start_outbound().await });
        Self::new(tx, join_handle)
    }

    fn start_inbound(
        socket: TcpStream,
        mut session: PeerSession,
        tx: peer::Sender,
    ) -> Self {
        let join_handle =
            task::spawn(async move { session.start_inbound(socket).await });
        Self::new(tx, join_handle)
    }

    fn new(
        tx: peer::Sender,
        join_handle: task::JoinHandle<peer::error::Result<()>>,
    ) -> Self {
        Self {
            tx: Some(tx),
            id: None,
            state: SessionState {
                connection: ConnectionState::Connecting,
                ..Default::default()
            },
            piece_count: 0,
            thruput: Default::default(),
            join_handle: Some(join_handle),
        }
    }
}

/// Contains the tracker client as well as additional metadata about the
/// tracker.
struct TrackerEntry {
    client: Tracker,
    /// If a previous announce contained a tracker_id, it should be included in
    /// next announces. Therefore it is cached here.
    id: Option<String>,
    /// The last announce time is kept here so that we don't request too often.
    last_announce_time: Option<Instant>,
    /// The interval at which we should update the tracker of our progress.
    /// This is set after the first announce request.
    interval: Option<Duration>,
    /// The absolute minimum interval at which we can contact tracker.
    /// This is set after the first announce request.
    min_interval: Option<Duration>,
    /// Each time we fail to requet from tracker, this counter is incremented.
    /// If it fails too often, we stop requesting from tracker.
    error_count: usize,
}

impl TrackerEntry {
    fn new(client: Tracker) -> Self {
        Self {
            client,
            id: None,
            last_announce_time: None,
            interval: None,
            min_interval: None,
            error_count: 0,
        }
    }

    /// Determines whether we should announce to the tracker at the given time,
    /// based on when we last announced.
    ///
    /// Later this function should take into consideration the client's minimum
    /// announce frequency settings.
    fn should_announce(
        &self,
        t: Instant,
        default_announce_interval: Duration,
    ) -> bool {
        if let Some(last_announce_time) = self.last_announce_time {
            let min_next_announce_time = last_announce_time
                + self.interval.unwrap_or(default_announce_interval);
            t > min_next_announce_time
        } else {
            true
        }
    }

    /// Determines whether we're allowed to announce at the given time.
    ///
    /// We may need peers before the next step in the announce interval.
    /// However, we can't do this too often, so we need to check our last
    /// announce time first.
    fn can_announce(
        &self,
        t: Instant,
        default_announce_interval: Duration,
    ) -> bool {
        if let Some(last_announce_time) = self.last_announce_time {
            let min_next_announce_time = last_announce_time
                + self
                .min_interval
                .or(self.min_interval)
                .unwrap_or(default_announce_interval);
            t > min_next_announce_time
        } else {
            true
        }
    }
}
