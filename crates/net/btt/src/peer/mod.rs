//! This module defines the implementation of the BitTorrent peer wire protocol.
//!
//! The main type responsible for communication with a peer is
//! a [`PeerSession`]. Making use of the BitTorrent byte protocol codec
//! implementation, this type implements the high-level operations of the
//! protocol.
//!
//! Each peer session is spawned within a torrent and cannot be used without
//! one, due to making use of shared data in torrent.

use crate::{
    bitfield::BitField,
    block::{Block, BlockInfo},
    error::Error,
    info::PieceIndex,
    peer::error::PeerResult,
    proto::{PeerCodec, PeerMessage},
    sha1::PeerId,
    torrent::{self, TorrentContext},
};
use futures::{stream::SplitSink, SinkExt, StreamExt};
use std::{
    collections::HashSet,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    net::TcpStream,
    sync::{
        mpsc::{self, UnboundedReceiver, UnboundedSender},
        RwLock,
    },
    time,
};
use tokio_util::codec::{Framed, FramedParts};

pub(crate) mod error;

/// After this timeout if the peers haven't become intereseted in each other,
/// the connection is severed.
const INACTIVITY_TIMEOUT: Duration = Duration::from_secs(60);

/// The channel on which torrent can send a command to the peer session task.
pub(crate) type Sender = UnboundedSender<Command>;
type Receiver = UnboundedReceiver<Command>;

/// A stopped or active connection with another BitTorrent peer.
///
/// This entity implements the BitTorrent wire protocol: it is responsible for
/// exchanging the BitTorrent messages that drive a download.
/// It only concerns itself with the network aspect of things: disk IO, for
/// example, is delegated to the [disk task](crate::disk::Disk).
///
/// A peer session may be started in two modes:
/// - outbound: for connecting to another BitTorrent peer;
/// - inbound: for starting a session from an existing incoming TCP connection.
///
/// The only difference in the above two is how the handshake is handled at the
/// beginning of the connection. From then on the session mechanisms are
/// identical.
///
/// # Important
///
/// For now only the BitTorrent v1 specification is implemented, without any
/// extensions.
pub(crate) struct PeerSession {
    /// Shared information of the torrent.
    torrent: Arc<TorrentContext>,

    /// The command channel on which peer session is being sent messages.
    ///
    /// A copy of this is kept within peer session as disk block reads are
    /// communicated back to session directly via its command port. For this, we
    /// need to pass a copy of the sender with each block read to the disk task.
    cmd_tx: Sender,
    /// The port on which peer session receives commands.
    cmd_rx: Receiver,

    /// Information about the peer.
    peer: PeerInfo,

    // /// Most of the session's information and state is stored here, i.e. it's
    // /// the "context" of the session.
    // ctx: SessionContext,
    /// Our pending requests that we sent to peer. It represents the blocks that
    /// we are expecting.
    ///
    /// If we receive a block whose request entry is here, that entry is
    /// removed. A request is also removed here when it is timed out.
    ///
    /// This structure's main purpose is for the timeout logic: if the peer
    /// times out, the currently pending blocks requested from peer are freed
    /// for other peer sessions to download. The block requests are not actually
    /// cancelled, since the blocks will usually arrive after some time, but we
    /// want to give other sessions a chance to download it faster. Therefore
    /// some blocks may arrive twice, in which case we drop the second arrival:
    /// this is not a problem, we just log and discard the second block.
    ///
    /// This will lead to overall better download performance as discarding all
    /// the blocks the timed out out peer would eventually send would be highly
    /// wasteful. In practice even with fast seeds occasional timeouts occur, at
    /// which point we may have hundreds if not thousands of pending blocks in
    /// the pipeline.
    /// Cancelling and subsequently rejecting them would be extremely wasteful.
    /// Whereas with this approach, we collect all those blocks when they do
    /// eventually arrive. According to manual tests most of these blocks won't
    /// actually be re-downloaded by other peers, so only a fraction of those
    /// will be wasted. Thus this method avoids bandwidth waste and cuts down
    /// overall download times.
    ///
    /// Since the Fast extension is not supported (yet), this is emptied when
    /// we're choked, as in that case we don't expect outstanding requests to be
    /// served.
    ///
    /// Note that if a reuest for a piece's block is in this queue, there _must_
    /// be a corresponding entry for the piece download in `downloads`.
    // TODO(https://github.com/mandreyel/cratetorrent/issues/11): Can we store
    // this information in just PieceDownload so that we don't have to enforce
    // this invariant (keeping in mind that later PieceDownloads will be shared
    // among PeerSessions)?
    outgoing_requests: HashSet<BlockInfo>,
    /// The requests we got from peer.
    ///
    /// The request's entry is removed from here when the block is transmitted
    /// or when the peer cancels it. If a peer sends a request and cancels it
    /// before the disk read is done, the read block is dropped.
    incoming_requests: HashSet<BlockInfo>,
}

/// Information about the peer we're connected to.
#[derive(Debug)]
pub(super) struct PeerInfo {
    /// The IP-port pair of the peer.
    pub addr: SocketAddr,
    /// Peer's 20 byte BitTorrent id. Updated when the peer sends us its peer
    /// id, in the handshake.
    pub id: Option<PeerId>,
    /// All pieces peer has, updated when it announces to us a new piece.
    ///
    /// Defaults to all pieces set to missing.
    pub pieces: BitField,
    /// A cache of the pieces that the peer has available.
    ///
    /// This is equivalent to `self.pieces.count_ones()` and is updated every
    /// time the peer sends us an announcement of a new piece.
    pub piece_count: usize,
}

impl PeerSession {
    /// Creates a new session with the peer at the given address.
    ///
    /// # Important
    ///
    /// This constructor only initializes the session components but does not
    /// actually start it. See [`Self::start`].
    pub fn new(torrent: Arc<TorrentContext>, addr: SocketAddr) -> (Self, Sender) {
        // let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        // let piece_count = torrent.storage.piece_count;
        // let log_target =
        //     format!("cratetorrent::peer [{}][{}]", torrent.id, addr);
        // (
        //     Self {
        //         torrent,
        //         cmd_tx: cmd_tx.clone(),
        //         cmd_rx,
        //         peer: PeerInfo {
        //             addr,
        //             pieces: BitField::repeat(false, piece_count),
        //             piece_count: 0,
        //             id: Default::default(),
        //         },
        //         ctx: SessionContext {
        //             log_target,
        //             ..SessionContext::default()
        //         },
        //         outgoing_requests: HashSet::new(),
        //         incoming_requests: HashSet::new(),
        //     },
        //     cmd_tx,
        // )
        todo!()
    }

    /// Starts an outbound peer session.
    ///
    /// This method tries to connect to the peer at the address given in the
    /// constructor, send a handshake, and start the session.
    /// It returns if the connection is closed or an error occurs.
    pub async fn start_outbound(&mut self) -> PeerResult<()> {
        // log::info!(target: &self.ctx.log_target, "Starting outbound session");
        //
        // // establish the TCP connection
        // log::info!(target: &self.ctx.log_target, "Connecting to peer");
        // self.ctx.set_connection_state(ConnectionState::Connecting);
        // let socket = TcpStream::connect(self.peer.addr).await?;
        // log::info!(target: &self.ctx.log_target, "Connected to peer");
        //
        // let socket = Framed::new(socket, HandshakeCodec);
        // self.start(socket, Direction::Outbound).await
        todo!()
    }

    /// Starts an inbound peer session from an existing TCP connection.
    ///
    /// The method waits for the peer to send its handshake, responds
    /// with a handshake, and starts the session.
    /// It returns if the connection is closed or an error occurs.
    pub async fn start_inbound(&mut self, socket: TcpStream) -> PeerResult<()> {
        // log::info!(target: &self.ctx.log_target, "Starting inbound session");
        // self.ctx.set_connection_state(ConnectionState::Connecting);
        // let socket = Framed::new(socket, HandshakeCodec);
        // self.start(socket, Direction::Inbound).await
        todo!()
    }

    /// Helper method for the common steps of setting up a session.
    async fn start(
        &mut self,
        mut socket: Framed<TcpStream, PeerCodec>,
        direction: Direction,
    ) -> PeerResult<()> {
        // self.ctx.set_connection_state(ConnectionState::Handshaking);
        //
        // // if this is an outbound connection, we have to send the first
        // // handshake
        // if direction == Direction::Outbound {
        //     let handshake =
        //         Handshake::new(self.torrent.info_hash, self.torrent.client_id);
        //     log::info!(target: &self.ctx.log_target, "Sending handshake");
        //     self.ctx.counters.protocol.up += handshake.len();
        //     socket.send(handshake).await?;
        // }
        //
        // // receive peer's handshake
        // log::info!(target: &self.ctx.log_target, "Waiting for peer handshake");
        // if let Some(peer_handshake) = socket.next().await {
        //     let peer_handshake = peer_handshake?;
        //     log::info!(target: &self.ctx.log_target, "Peer sent handshake");
        //     log::trace!(target: &self.ctx.log_target, "Peer handshake: {:?}", peer_handshake);
        //     // codec should only return handshake if the protocol string in it
        //     // is valid
        //     debug_assert_eq!(peer_handshake.prot, PROTOCOL_STRING.as_bytes());
        //
        //     self.ctx.counters.protocol.down += peer_handshake.len();
        //
        //     // verify that the advertised torrent info hash is the same as ours
        //     if peer_handshake.info_hash != self.torrent.info_hash {
        //         log::info!(target: &self.ctx.log_target, "Peer handshake invalid info hash");
        //         // abort session, info hash is invalid
        //         return Err(PeerError::InvalidInfoHash);
        //     }
        //
        //     // set the peer's id
        //     self.peer.id = Some(peer_handshake.peer_id);
        //
        //     // if this is an inbound connection, we reply with the handshake
        //     if direction == Direction::Inbound {
        //         let handshake = Handshake::new(
        //             self.torrent.info_hash,
        //             self.torrent.client_id,
        //         );
        //         log::info!(target: &self.ctx.log_target, "Sending handshake");
        //         self.ctx.counters.protocol.up += handshake.len();
        //         socket.send(handshake).await?;
        //     }
        //
        //     // now that we have the handshake, we need to switch to the peer
        //     // message codec and save the socket in self (note that we need to
        //     // keep the buffer from the original codec as it may contain bytes
        //     // of any potential message the peer may have sent after the
        //     // handshake)
        //     let old_parts = socket.into_parts();
        //     let mut new_parts = FramedParts::new(old_parts.io, PeerCodec);
        //     // reuse buffers of previous codec
        //     new_parts.read_buf = old_parts.read_buf;
        //     new_parts.write_buf = old_parts.write_buf;
        //     let socket = Framed::from_parts(new_parts);
        //
        //     // update torrent of connection
        //     self.torrent.cmd_tx.send(torrent::Command::PeerConnected {
        //         addr: self.peer.addr,
        //         id: peer_handshake.peer_id,
        //     })?;
        //
        //     // enter the piece availability exchange state
        //     self.ctx
        //         .set_connection_state(ConnectionState::AvailabilityExchange);
        //     log::info!(target: &self.ctx.log_target, "Session state: {:?}",
        // self.ctx.state.connection);
        //
        //     // run the session
        //     if let Err(e) = self.run(socket).await {
        //         log::error!(
        //             target: &self.ctx.log_target,
        //             "Session stopped due to an error: {}",
        //             e
        //         );
        //         self.ctx.set_connection_state(ConnectionState::Disconnected);
        //         self.torrent.cmd_tx.send(torrent::Command::PeerState {
        //             addr: self.peer.addr,
        //             info: self.session_info(),
        //         })?;
        //         self.torrent.alert_tx.send(Alert::Error(Error::Peer {
        //             id: self.torrent.id,
        //             addr: self.peer.addr,
        //             error: e,
        //         }))?;
        //     }
        // } else {
        //     log::error!(target: &self.ctx.log_target, "No handshake received");
        //     self.ctx.set_connection_state(ConnectionState::Disconnected);
        //     self.torrent.cmd_tx.send(torrent::Command::PeerState {
        //         addr: self.peer.addr,
        //         info: self.session_info(),
        //     })?;
        // }
        //
        // // session exited as a result of a clean shutdown or an error, perform
        // // some cleanup before exiting
        //
        // // cancel any pending requests to not block other peers from completing
        // // the piece
        // if !self.outgoing_requests.is_empty() {
        //     log::info!(
        //         target: &self.ctx.log_target,
        //         "Cancelling remaining {} request(s)",
        //         self.outgoing_requests.len()
        //     );
        //     self.free_pending_blocks().await;
        // }
        //
        // // send a state update message to torrent to actualize possible download
        // // stats changes
        // self.ctx.set_connection_state(ConnectionState::Disconnected);
        // self.torrent.cmd_tx.send(torrent::Command::PeerState {
        //     addr: self.peer.addr,
        //     info: self.session_info(),
        // })?;

        Ok(())
    }

    /// Runs the session after connection to peer is established.
    ///
    /// This is the main session "loop" and performs the core of the session
    /// logic: exchange of messages, timeout logic, etc.
    async fn run(&mut self, socket: Framed<TcpStream, PeerCodec>) -> PeerResult<()> {
        // self.ctx.connected_time = Some(Instant::now());
        //
        // // split the sink and stream so that we can pass the sink while holding
        // // a reference to the stream in the loop
        // let (mut sink, mut stream) = socket.split();
        //
        // // This is the beginning of the session, which is the only time
        // // a peer is allowed to advertise their pieces. If we have pieces
        // // available, send a bitfield message.
        // {
        //     let piece_picker_guard = self.torrent.piece_picker.read().await;
        //     let own_pieces = piece_picker_guard.own_pieces();
        //     if own_pieces.any() {
        //         log::info!(target: &self.ctx.log_target, "Sending piece availability");
        //         sink.send(PeerMessage::BitField(own_pieces.clone())).await?;
        //         log::info!(target: &self.ctx.log_target, "Sent piece availability");
        //     }
        // }
        //
        // // used for collecting session stats every second
        // let mut tick_timer = time::interval(Duration::from_secs(1));
        //
        // // start the loop for receiving messages from peer and commands from
        // // other parts of the engine
        // loop {
        //     tokio::select! {
        //         now = tick_timer.tick() => {
        //             self.tick(&mut sink, now.into_std()).await?;
        //         }
        //         Some(msg) = stream.next() => {
        //             let msg = msg?;
        //
        //             // handle bitfield message separately as it may only be
        //             // received directly after the handshake (later once we
        //             // implement the FAST extension, there will be other piece
        //             // availability related messages to handle)
        //             if self.ctx.state.connection == ConnectionState::AvailabilityExchange {
        //                 if let PeerMessage::BitField(bitfield) = msg {
        //                     self.handle_bitfield_msg(&mut sink, bitfield).await?;
        //                 } else {
        //                     // it's not mandatory to send a bitfield message
        //                     // right after the handshake
        //                     self.handle_msg(&mut sink, msg).await?;
        //                 }
        //
        //                 // if neither of us have any pieces, disconnect, there
        //                 // is no point in keeping the connection alive
        //                 if self
        //                     .torrent
        //                     .piece_picker
        //                     .read()
        //                     .await
        //                     .own_pieces()
        //                     .not_any()
        //                     && self.peer.pieces.not_any()
        //                 {
        //                     log::warn!(
        //                         target: &self.ctx.log_target,
        //                         "Neither side of connection has any pieces, disconnecting"
        //                     );
        //                     return Ok(());
        //                 }
        //
        //                 // enter connected state
        //                 self.ctx.set_connection_state(ConnectionState::Connected);
        //                 log::info!(target: &self.ctx.log_target,
        //                     "Session state: {:?}",
        //                     self.ctx.state.connection
        //                 );
        //             } else {
        //                 self.handle_msg(&mut sink, msg).await?;
        //             }
        //         }
        //         Some(cmd) = self.cmd_rx.recv() => {
        //             match cmd {
        //                 Command::Block(block)=> {
        //                     self.send_block(&mut sink, block).await?;
        //                 }
        //                 Command::PieceCompletion { index, in_endgame } => {
        //                     self.ctx.in_endgame = in_endgame;
        //                     self.handle_piece_completion(&mut sink, index).await?;
        //                 }
        //                 Command::Shutdown => {
        //                     log::info!(
        //                         target: &self.ctx.log_target,
        //                         "Shutting down session"
        //                     );
        //                     break;
        //                 }
        //             }
        //         }
        //     }
        // }

        Ok(())
    }

    /// The session tick, as in "the tick of a clock", which runs every second
    /// to perform periodic updates.
    ///
    /// This is when we update statistics and report them to torrent (and later
    /// perhaps to the user directly, if requested), when the session leaves
    /// slow-start, when it checks various timeouts, and when it updates the
    /// target request queue size.
    async fn tick(
        &mut self,
        sink: &mut SplitSink<Framed<TcpStream, PeerCodec>, PeerMessage>,
        now: Instant,
    ) -> PeerResult<()> {
        // // if we haven't become interested in each other for too long,
        // // disconnect
        // if !self.ctx.state.is_interested
        //     && !self.ctx.state.is_peer_interested
        //     && now.saturating_duration_since(
        //     self.ctx.connected_time.expect("not connected"),
        // ) >= INACTIVITY_TIMEOUT
        // {
        //     log::warn!(target: &self.ctx.log_target, "Not interested in each other,
        // disconnecting");     return Err(PeerError::InactivityTimeout);
        // }
        //
        // // resent requests if we have pending requests and more time has elapsed
        // // since the last request than the current timeout value
        // if !self.outgoing_requests.is_empty() {
        //     self.check_request_timeout(sink).await?;
        // }
        //
        // // TODO(https://github.com/mandreyel/cratetorrent/issues/42): send
        // // keep-alive
        //
        // // if there was any state change, notify torrent
        // if self.ctx.changed {
        //     log::debug!(
        //         target: &self.ctx.log_target,
        //         "State changed, updating torrent",
        //     );
        //     self.torrent.cmd_tx.send(torrent::Command::PeerState {
        //         addr: self.peer.addr,
        //         info: self.session_info(),
        //     })?;
        // }
        //
        // // update session context
        // let prev_queue_len = self.ctx.target_request_queue_len;
        // self.ctx.tick();
        // if let (Some(prev_queue_len), Some(curr_queue_len)) =
        // (prev_queue_len, self.ctx.target_request_queue_len)
        // {
        //     if prev_queue_len != curr_queue_len {
        //         log::info!(
        //             target: &self.ctx.log_target,
        //             "Request queue changed from {} to {}",
        //             prev_queue_len,
        //             curr_queue_len
        //         );
        //     }
        // }
        //
        // log::debug!(
        //     target: &self.ctx.log_target,
        //     "Stats: \
        //     download: {dl_rate} b/s (peak: {dl_peak} b/s, total: {dl_total} b), \
        //     pending: {out_req}, queue: {queue}, rtt: {rtt_ms} ms (~{rtt_s} s), \
        //     waste: {waste},
        //     upload: {ul_rate} b/s (peak: {ul_peak} b/s, total: {ul_total} b), \
        //     pending: {in_req}",
        //     dl_rate = self.ctx.counters.payload.down.avg(),
        //     dl_peak = self.ctx.counters.payload.down.peak(),
        //     dl_total = self.ctx.counters.payload.down.total(),
        //     out_req = self.outgoing_requests.len(),
        //     queue = self.ctx.target_request_queue_len.unwrap_or_default(),
        //     rtt_ms = self.ctx.avg_request_rtt.mean().as_millis(),
        //     rtt_s = self.ctx.avg_request_rtt.mean().as_secs(),
        //     waste = self.ctx.counters.waste.total(),
        //     ul_rate = self.ctx.counters.payload.up.avg(),
        //     ul_peak = self.ctx.counters.payload.up.peak(),
        //     ul_total = self.ctx.counters.payload.up.total(),
        //     in_req = self.incoming_requests.len(),
        // );

        Ok(())
    }

    /// Times out the peer if it hasn't sent a request in too long.
    async fn check_request_timeout(
        &mut self,
        sink: &mut SplitSink<Framed<TcpStream, PeerCodec>, PeerMessage>,
    ) -> PeerResult<()> {
        // if let Some(last_outgoing_request_time) =
        // self.ctx.last_outgoing_request_time
        // {
        //     let elapsed_since_last_request = Instant::now()
        //         .saturating_duration_since(last_outgoing_request_time);
        //     let request_timeout = self.ctx.request_timeout();
        //
        //     log::debug!(
        //         target: &self.ctx.log_target,
        //         "Checking request timeout \
        //         (last {} ms ago, timeout: {} ms)",
        //         elapsed_since_last_request.as_millis(),
        //         request_timeout.as_millis()
        //     );
        //
        //     if elapsed_since_last_request > request_timeout {
        //         log::warn!(
        //             target: &self.ctx.log_target,
        //             "Timeout after {} ms, cancelling {} request(s) (timeouts: {})",
        //             elapsed_since_last_request.as_millis(),
        //             self.outgoing_requests.len(),
        //             self.ctx.timed_out_request_count + 1
        //         );
        //
        //         // Cancel all requests and re-issue a single one (since we can
        //         // only request a single block now). Start by freeing up the
        //         // blocks in their piece download.
        //         // Note that we're not telling the peer that we timed out the
        //         // request so that if it arrives some time later and is not
        //         // requested by another peer, we can still collect it.
        //         self.free_pending_blocks().await;
        //         self.ctx.register_request_timeout();
        //         self.make_requests(sink).await?;
        //     }
        // }

        Ok(())
    }

    /// Marks requested blocks as free in their respective downlaods so that
    /// other peer sessions may download them.
    async fn free_pending_blocks(&mut self) {
        // let downloads_guard = self.torrent.downloads.read().await;
        // for block in self.outgoing_requests.drain() {
        //     // The piece may no longer be present if it was compoleted by
        //     // another peer in the meantime and torrent removed it from the
        //     // shared download store. This is fine, in this case we don't have
        //     // anything to do.
        //     if let Some(download) = downloads_guard.get(&block.piece_index) {
        //         log::debug!(
        //             target: &self.ctx.log_target,
        //             "Freeing block {} for download",
        //             block
        //         );
        //         download.write().await.free_block(&block);
        //     }
        // }
    }

    /// Returns a summary of the most important information of the session
    /// state to send to torrent.
    fn session_info(&self) -> SessionTick {
        // SessionTick {
        //     state: self.ctx.state,
        //     counters: self.ctx.counters,
        //     piece_count: self.peer.piece_count,
        // }
        todo!()
    }

    /// Handles a message expected in the `AvailabilityExchange` state
    /// (currently only the bitfield message).
    async fn handle_bitfield_msg(
        &mut self,
        sink: &mut SplitSink<Framed<TcpStream, PeerCodec>, PeerMessage>,
        mut bitfield: BitField,
    ) -> PeerResult<()> {
        // log::info!(target: &self.ctx.log_target, "Handling peer BitField message");
        // log::trace!(target: &self.ctx.log_target, "BitField: {:?}", bitfield);
        //
        // debug_assert_eq!(
        //     self.ctx.state.connection,
        //     ConnectionState::AvailabilityExchange
        // );
        //
        // // The bitfield raw data that is sent over the wire may be longer than
        // // the logical pieces it represents, if there the number of pieces in
        // // torrent is not a multiple of 8. Therefore, we need to slice off the
        // // last part of the bitfield.
        // //
        // // According to the spec if the remainder contains any non-zero
        // // bits, we need to abort the connection. Not sure if this is too
        // // strict, there doesn't seem much harm in it so we skip the check.
        // bitfield.resize(self.torrent.storage.piece_count, false);
        //
        // // register peer's pieces with piece picker and determine interest in it
        // let is_interested = self
        //     .torrent
        //     .piece_picker
        //     .write()
        //     .await
        //     .register_peer_pieces(&bitfield);
        // self.peer.pieces = bitfield;
        // self.peer.piece_count = self.peer.pieces.count_ones();
        // if self.peer.piece_count == self.torrent.storage.piece_count {
        //     log::info!(target: &self.ctx.log_target, "Peer is a seed, interested: {}",
        // is_interested); } else {
        //     log::info!(
        //         target: &self.ctx.log_target,
        //         "Peer has {}/{} pieces, interested: {}",
        //         self.peer.piece_count,
        //         self.torrent.storage.piece_count,
        //         is_interested
        //     );
        // }
        //
        // // we may have become interested in peer
        // self.update_interest(sink, is_interested).await
        todo!()
    }

    /// Handles messages from peer that are expected in the `Connected` state.
    async fn handle_msg(
        &mut self,
        sink: &mut SplitSink<Framed<TcpStream, PeerCodec>, PeerMessage>,
        msg: PeerMessage,
    ) -> PeerResult<()> {
        // // record protocol message size
        // self.ctx.counters.protocol.down += msg.protocol_len();
        // match msg {
        //     PeerMessage::BitField(_) => {
        //         log::info!(
        //             target: &self.ctx.log_target,
        //             "Peer sent bitfield message not after handshake"
        //         );
        //         return Err(PeerError::BitfieldNotAfterHandshake);
        //     }
        //     PeerMessage::KeepAlive => {
        //         log::info!(target: &self.ctx.log_target, "Peer sent keep alive");
        //     }
        //     PeerMessage::Choke => {
        //         if !self.ctx.state.is_choked {
        //             log::info!(target: &self.ctx.log_target, "Peer choked us");
        //             // since we're choked we don't expect to receive blocks
        //             // for our pending requests and free them for other peers to
        //             // download
        //             self.free_pending_blocks().await;
        //             self.ctx.update_state(|state| state.is_choked = true);
        //         }
        //     }
        //     PeerMessage::Unchoke => {
        //         if self.ctx.state.is_choked {
        //             log::info!(target: &self.ctx.log_target, "Peer unchoked us");
        //             self.ctx.update_state(|state| state.is_choked = false);
        //
        //             // if we're interested, start sending requests
        //             if self.ctx.state.is_interested {
        //                 self.ctx.prepare_for_download();
        //                 // now that we are allowed to request blocks, start the
        //                 // download pipeline if we're interested
        //                 self.make_requests(sink).await?;
        //             }
        //         }
        //     }
        //     PeerMessage::Interested => {
        //         if !self.ctx.state.is_peer_interested {
        //             // TODO(https://github.com/mandreyel/cratetorrent/issues/60):
        //             // we currently unchkoe peer unconditionally, but we should
        //             // implement the proper unchoke algorithm in `Torrent`
        //             log::info!(target: &self.ctx.log_target, "Peer became interested");
        //             log::info!(target: &self.ctx.log_target, "Unchoking peer");
        //             self.ctx.update_state(|state| {
        //                 state.is_peer_interested = true;
        //                 state.is_peer_choked = false;
        //             });
        //             sink.send(PeerMessage::Unchoke).await?;
        //         }
        //     }
        //     PeerMessage::NotInterested => {
        //         if self.ctx.state.is_peer_interested {
        //             log::info!(target: &self.ctx.log_target, "Peer no longer interested");
        //             self.ctx.update_state(|state| {
        //                 state.is_peer_interested = false;
        //             });
        //         }
        //     }
        //     PeerMessage::Block {
        //         piece_index,
        //         offset,
        //         data,
        //     } => {
        //         let block_info = BlockInfo {
        //             piece_index,
        //             offset,
        //             len: data.len() as u32,
        //         };
        //         self.handle_block_msg(block_info, data.into_owned()).await?;
        //
        //         // we may be able to make more requests now that a block has
        //         // arrived
        //         self.make_requests(sink).await?;
        //     }
        //     PeerMessage::Request(block_info) => {
        //         self.handle_request_msg(block_info).await?;
        //     }
        //     PeerMessage::Have { piece_index } => {
        //         self.handle_have_msg(sink, piece_index).await?;
        //     }
        //     PeerMessage::Cancel(block_info) => {
        //         // before processing request validate block info
        //         self.validate_block_info(&block_info)?;
        //         log::info!(target: &self.ctx.log_target, "Peer cancelled block {}", block_info);
        //         self.incoming_requests.remove(&block_info);
        //     }
        // }

        Ok(())
    }

    /// Fills the session's download pipeline with the optimal number of
    /// requests.
    ///
    /// To see what this means, please refer to the
    /// `Status::best_request_queue_len` or the relevant section in DESIGN.md.
    async fn make_requests(
        &mut self,
        sink: &mut SplitSink<Framed<TcpStream, PeerCodec>, PeerMessage>,
    ) -> PeerResult<()> {
        // log::trace!(target: &self.ctx.log_target, "Making requests");
        //
        // if self.ctx.state.is_choked {
        //     log::debug!(target: &self.ctx.log_target, "Cannot make requests while choked");
        //     return Ok(());
        // }
        //
        // if !self.ctx.state.is_interested {
        //     log::debug!(target: &self.ctx.log_target, "Cannot make requests if not interested");
        //     return Ok(());
        // }
        //
        // // TODO: optimize this by using the preallocated hashset in self
        // let mut requests = Vec::new();
        // let target_request_queue_len =
        //     self.ctx.target_request_queue_len.unwrap_or_default();
        //
        // // If we have active downloads, prefer to continue those. This will
        // // result in less in-progress pieces.
        // for download in self.torrent.downloads.write().await.values_mut() {
        //     // check and calculate the number of requests we can make now
        //     let outgoing_request_count =
        //         requests.len() + self.outgoing_requests.len();
        //     // our outgoing request queue shouldn't exceed the allowed request
        //     // queue size
        //     if outgoing_request_count >= target_request_queue_len {
        //         break;
        //     }
        //     let to_request_count =
        //         target_request_queue_len - outgoing_request_count;
        //
        //     let mut download_write_guard = download.write().await;
        //     log::trace!(
        //         target: &self.ctx.log_target,
        //         "Trying to continue download {}",
        //         download_write_guard.piece_index()
        //     );
        //     download_write_guard.pick_blocks(
        //         to_request_count,
        //         &mut requests,
        //         self.ctx.in_endgame,
        //         &self.outgoing_requests,
        //     );
        // }
        //
        // // while we can make more requests we start new download(s)
        // loop {
        //     let outgoing_request_count =
        //         requests.len() + self.outgoing_requests.len();
        //     // our outgoing request queue shouldn't exceed the allowed request
        //     // queue size
        //     if outgoing_request_count >= target_request_queue_len {
        //         break;
        //     }
        //     let to_request_count =
        //         target_request_queue_len - outgoing_request_count;
        //
        //     log::debug!(target: &self.ctx.log_target, "Trying to pick new piece");
        //
        //     if let Some(index) =
        //     self.torrent.piece_picker.write().await.pick_piece()
        //     {
        //         log::info!(target: &self.ctx.log_target, "Picked piece {}", index);
        //
        //         let mut download = PieceDownload::new(
        //             index,
        //             self.torrent.storage.piece_len(index),
        //         );
        //
        //         download.pick_blocks(
        //             to_request_count,
        //             &mut requests,
        //             self.ctx.in_endgame,
        //             &self.outgoing_requests,
        //         );
        //         // save download
        //         self.torrent
        //             .downloads
        //             .write()
        //             .await
        //             .insert(index, RwLock::new(download));
        //     } else {
        //         log::debug!(
        //             target: &self.ctx.log_target,
        //             "Cannot pick more pieces (pending \
        //             pieces: {}, blocks: {})",
        //             self.torrent.downloads.read().await.len(),
        //             self.outgoing_requests.len(),
        //         );
        //
        //         break;
        //     }
        // }
        //
        // if !requests.is_empty() {
        //     log::info!(
        //         target: &self.ctx.log_target,
        //         "Requesting {} block(s) ({} pending)",
        //         requests.len(),
        //         self.outgoing_requests.len()
        //     );
        //     self.ctx.last_outgoing_request_time = Some(Instant::now());
        //     // make the actual requests
        //     for req in requests.into_iter() {
        //         log::debug!(target: &self.ctx.log_target, "Requesting block {}", req);
        //         self.outgoing_requests.insert(req);
        //         // TODO: batch these in a single syscall, or is this already
        //         // being done by the tokio codec type?
        //         sink.send(PeerMessage::Request(req)).await?;
        //         self.ctx.counters.protocol.up +=
        //             MessageId::Request.header_len();
        //     }
        // }

        Ok(())
    }

    /// Verifies block validity, registers the download, and records statistics.
    ///
    /// Blocks are accepted even if timed out/cancelled, if the block has not
    /// been downloaded (e.g. from another peer) since then.
    async fn handle_block_msg(&mut self, block_info: BlockInfo, data: Vec<u8>) -> PeerResult<()> {
        // // remove pending block request
        // self.outgoing_requests.remove(&block_info);
        //
        // // try to find the piece to which this block corresponds
        // // and mark the block in piece as downloaded
        // let prev_status = match self
        //     .torrent
        //     .downloads
        //     .read()
        //     .await
        //     .get(&block_info.piece_index)
        // {
        //     Some(download) => {
        //         download.write().await.received_block(&block_info)
        //     }
        //     None => {
        //         // silently ignore this block if we didn't expected it
        //         //
        //         // TODO(https://github.com/mandreyel/cratetorrent/issues/10): In
        //         // the future we could add logic that only accepts blocks within
        //         // a window after the last request. If not done, peer could DoS
        //         // us by sending unwanted blocks repeatedly.
        //         log::warn!(
        //             target: &self.ctx.log_target,
        //             "Discarding block {} with no piece download{}",
        //             block_info,
        //             if self.ctx.in_endgame {
        //                 " in endgame"
        //             } else {
        //                 ""
        //             }
        //         );
        //         self.ctx.record_waste(block_info.len);
        //         return Ok(());
        //     }
        // };
        //
        // // don't process the block if already downloaded
        // if prev_status == BlockStatus::Received {
        //     self.ctx.record_waste(block_info.len);
        //     log::info!(
        //         target: &self.ctx.log_target,
        //         "Already downloaded block {}",
        //         block_info
        //     );
        // } else {
        //     log::info!(
        //         target: &self.ctx.log_target,
        //         "Got block {}{}",
        //         block_info,
        //         if self.ctx.in_slow_start {
        //             " in slow-start"
        //         } else if self.ctx.in_endgame {
        //             " in endgame"
        //         } else {
        //             ""
        //         }
        //     );
        //
        //     // update download stats
        //     self.ctx.update_download_stats(block_info.len);
        //
        //     // validate and save the block to disk by sending a write command to the
        //     // disk task
        //     self.torrent.disk_tx.send(disk::Command::WriteBlock {
        //         id: self.torrent.id,
        //         block_info,
        //         data,
        //     })?;
        // }

        Ok(())
    }

    /// Handles the peer request message.
    ///
    /// If the request is valid and that peer may make requests, we instruct the
    /// disk task to fetch the block from disk. Later, when the disk is fetched,
    /// we receive a message on the peer session's command port in
    /// [`Self::run`]. This is when the block is actually sent to peer, if by
    /// the request is not cancelled by then.
    async fn handle_request_msg(&mut self, block_info: BlockInfo) -> PeerResult<()> {
        // log::info!(target: &self.ctx.log_target, "Got request: {:?}", block_info);
        //
        // // before processing request validate block info
        // self.validate_block_info(&block_info)?;
        //
        // // check if peer is not choked: if they are, they can't request blocks
        // if self.ctx.state.is_peer_choked {
        //     log::warn!(target: &self.ctx.log_target, "Choked peer sent request");
        //     return Err(PeerError::RequestWhileChoked);
        // }
        //
        // // check if peer is not already requesting this block
        // if self.incoming_requests.contains(&block_info) {
        //     // TODO: if peer keeps spamming us, close connection
        //     log::warn!(target: &self.ctx.log_target, "Peer sent duplicate block request");
        //     return Ok(());
        // }
        //
        // log::info!(target: &self.ctx.log_target, "Issuing disk IO read for block {}",
        // block_info); self.incoming_requests.insert(block_info);
        //
        // // validate and save the block to disk by sending a write command to the
        // // disk task
        // self.torrent.disk_tx.send(disk::Command::ReadBlock {
        //     id: self.torrent.id,
        //     block_info,
        //     result_tx: self.cmd_tx.clone(),
        // })?;

        Ok(())
    }

    /// Sends the block to peer if the peer still wants it (hasn't canceled the
    /// request).
    async fn send_block(
        &mut self,
        sink: &mut SplitSink<Framed<TcpStream, PeerCodec>, PeerMessage>,
        block: Block,
    ) -> PeerResult<()> {
        // let info = block.info();
        // log::info!(target: &self.ctx.log_target, "Read from disk {}", info);
        //
        // // remove peer's pending request
        // let was_present = self.incoming_requests.remove(&info);
        //
        // // check if the request hasn't been canceled yet
        // if !was_present {
        //     log::warn!(target: &self.ctx.log_target, "No matching request entry for {}", info);
        //     return Ok(());
        // }
        //
        // // if it hasn't, send the data to peer
        // log::info!(target: &self.ctx.log_target, "Sending {}", info);
        // sink.send(PeerMessage::Block {
        //     piece_index: block.piece_index,
        //     offset: block.offset,
        //     data: block.data,
        // })
        //     .await?;
        // log::info!(target: &self.ctx.log_target, "Sent {}", info);
        //
        // // update download stats
        // self.ctx.update_upload_stats(info.len);

        Ok(())
    }

    /// Handles the announcement of a new piece that peer has. This may cause us
    /// to become interested in peer and start making requests.
    async fn handle_have_msg(
        &mut self,
        sink: &mut SplitSink<Framed<TcpStream, PeerCodec>, PeerMessage>,
        piece_index: PieceIndex,
    ) -> PeerResult<()> {
        // log::info!(target: &self.ctx.log_target, "Peer has piece {}", piece_index);
        //
        // // validate piece index
        // self.validate_piece_index(piece_index)?;
        //
        // // It's important to check if peer already has this piece.
        // // Otherwise we'd record duplicate pieces in the swarm in the below
        // // availability registration.
        // if self.peer.pieces[piece_index] {
        //     return Ok(());
        // }
        //
        // self.peer.pieces.set(piece_index, true);
        // self.peer.piece_count += 1;
        //
        // // need to recalculate interest with each received piece
        // let is_interested = self
        //     .torrent
        //     .piece_picker
        //     .write()
        //     .await
        //     .register_peer_piece(piece_index);
        //
        // // we may have become interested in peer
        // self.update_interest(sink, is_interested).await
        todo!()
    }

    /// Checks whether we have become or stopped being interested in the peer.
    async fn update_interest(
        &mut self,
        sink: &mut SplitSink<Framed<TcpStream, PeerCodec>, PeerMessage>,
        is_interested: bool,
    ) -> PeerResult<()> {
        // // we may have become interested in peer
        // if !self.ctx.state.is_interested && is_interested {
        //     log::info!(target: &self.ctx.log_target, "Became interested in peer");
        //     self.ctx.counters.protocol.up += MessageId::Interested.header_len();
        //     self.ctx.update_state(|state| {
        //         state.is_interested = is_interested;
        //     });
        //     // send interested message to peer
        //     sink.send(PeerMessage::Interested).await?;
        // } else if self.ctx.state.is_interested && !is_interested {
        //     log::info!(target: &self.ctx.log_target, "No longer interested in peer");
        //     self.ctx.update_state(|state| {
        //         state.is_interested = is_interested;
        //     });
        //     // TODO: do we need to do anything else here?
        // }

        Ok(())
    }

    /// Validates that the block info refers to a valid piece's valid block in
    /// torrent.
    fn validate_block_info(&self, info: &BlockInfo) -> PeerResult<()> {
        // log::trace!(target: &self.ctx.log_target, "Validating {}", info);
        // self.validate_piece_index(info.piece_index)?;
        // let piece_len = self.torrent.storage.piece_len(info.piece_index);
        // if info.len > 0 && info.offset + info.len <= piece_len {
        //     Ok(())
        // } else {
        //     log::warn!(target: &self.ctx.log_target, "Peer sent invalid {}", info);
        //     Err(PeerError::InvalidBlockInfo)
        // }
        todo!()
    }

    /// Validates that the index refers to a valid piece in torrent.
    fn validate_piece_index(&self, index: PieceIndex) -> PeerResult<()> {
        // if index < self.torrent.storage.piece_count {
        //     Ok(())
        // } else {
        //     log::warn!(
        //         target: &self.ctx.log_target,
        //         "Peer sent invalid piece index: {}",
        //         index
        //     );
        //     Err(PeerError::InvalidPieceIndex)
        // }
        todo!()
    }

    /// When the torrent completes a new piece, peer sessions are notified of
    /// it.
    ///
    /// If peer has the piece, we check if we had any requests for blocks in it
    /// that we need to cancel. If peer doesn't have the piece, we announce it.
    async fn handle_piece_completion(
        &mut self,
        sink: &mut SplitSink<Framed<TcpStream, PeerCodec>, PeerMessage>,
        piece_index: PieceIndex,
    ) -> PeerResult<()> {
        // // if peer doesn't have the piece, announce it
        // if !self.peer.pieces[piece_index] {
        //     log::debug!(
        //         target: &self.ctx.log_target,
        //         "Announcing piece {}",
        //         piece_index
        //     );
        //     sink.send(PeerMessage::Have { piece_index }).await?;
        // } else {
        //     // Otherwise peer has it and we may have requested it. Check if
        //     // there are any pending requests for blocks in this piece, and if
        //     // so, cancel them.
        //     // TODO: We could actually send the cancel messages much sooner,
        //     // when we first receive the block (rather then waiting for the
        //     // piece completion). However, it would require an mpsc roundtrip to
        //     // torrent and all other peers, for each of these blocks received in
        //     // endgame, so it is questionable whether it's worth it at the cost
        //     // of slowing down the engine.
        //     for block in self.outgoing_requests.iter() {
        //         if block.piece_index == piece_index {
        //             log::info!(
        //                 target: &self.ctx.log_target,
        //                 "Already have block {}, cancelling",
        //                 block
        //             );
        //             sink.send(PeerMessage::Cancel(*block)).await?;
        //         }
        //     }
        // }

        Ok(())
    }
}

/// The commands peer session can receive.
pub(crate) enum Command {
    /// The result of reading a block from disk.
    Block(Block),
    /// Notifies this peer session that a new piece is available.
    PieceCompletion {
        /// The piece that was completed.
        index: PieceIndex,
        /// Tell the session to enter endgame mode.
        in_endgame: bool,
    },
    /// Eventually shut down the peer session.
    Shutdown,
}

/// Events produced by this peer that will be reported to the torrent.
#[derive(Debug)]
pub(crate) enum PeerEvent {}

/// Determines who initiated the connection.
#[derive(Clone, Copy, PartialEq)]
enum Direction {
    Outbound,
    Inbound,
}

/// The most essential information of a peer session that is sent to torrent
/// with each session tick.
#[derive(Debug)]
pub(crate) struct SessionTick {
    // /// A snapshot of the session state.
    // pub state: SessionState,
    // /// Various transfer statistics.
    // pub counters: ThruputCounters,
    /// The number of pieces the peer has available.
    pub piece_count: usize,
}
