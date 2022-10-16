use crate::{
    alert::{AlertReceiver, AlertSender},
    bitfield::BitField,
    disk,
    disk::error::NewTorrentError,
    error::TorrentResult,
    info::{Metainfo, StorageInfo},
    sha1::{PeerId, RETH_TORRENT_CLIENT_ID},
    torrent,
    torrent::{config::TorrentConfig, Torrent, TorrentId},
    tracker::Tracker,
};
use futures::Stream;
use pin_project::pin_project;
use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{
    sync::{
        mpsc, oneshot,
        oneshot::{channel, Sender as OneshotSender},
    },
    task,
    task::JoinHandle,
};
use tracing::{error, info, trace};
use typed_builder::TypedBuilder;
use crate::alert::AlertStream;

/// Provides the API to interact with torrents.
#[derive(Debug)]
pub struct Bittorrent {
    /// Channel used to communicate with the [`BittorrentHandler`].
    command_tx: mpsc::UnboundedSender<BttCommand>,
    join_handle: Option<JoinHandle<TorrentResult<()>>>,
}

// === impl Bittorrent ===

impl Bittorrent {
    /// Creates a new instance of [`Bittorrent`] and the [`BittorrentHandler`] task.
    pub fn spawn(config: BittorrentConfig) -> TorrentResult<(Self, AlertStream)> {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (disk_join_handle, disk_tx) = disk::spawn(command_tx.clone())?;
        let (alert_tx, alert_rx) = mpsc::unbounded_channel();
        let mut handler = BittorrentHandler {
            torrents: Default::default(),
            command_rx,
            disk_tx,
            disk_join_handle: Some(disk_join_handle),
            config,
            alert_tx,
            next_id: 0,
        };

        let join_handle = task::spawn(async move { handler.run().await });

        Ok((Self { command_tx, join_handle: Some(join_handle) }, AlertStream::new(alert_rx)))
    }

    /// Creates and starts a torrent, if its metainfo is valid.
    ///
    /// If successful, it returns the id of the torrent. This id can be used to
    /// identify the torrent when issuing further commands to engine.
    pub async fn create_torrent(&self, params: TorrentParams) -> TorrentResult<TorrentId> {
        let (tx, rx) = oneshot::channel();
        self.command_tx.send(BttCommand::CreateTorrent { params, tx })?;
        Ok(rx.await?)
    }

    /// Gracefully shuts down the engine and waits for all its torrents to do
    /// the same.
    ///
    /// # Panics
    ///
    /// This method panics if the engine has already been shut down.
    pub async fn shutdown(mut self) -> TorrentResult<()> {
        Ok(())
    }
}

/// The receiver half of the [`Bittorrent`] instance.
///
/// This can create new torrents and is con communicate with the spawned tasks for torrents and the
/// disk manager. It will also bubble up
///
/// This needs to be spawned on another task, so it get process all commands and events
#[must_use = "Handler does nothing unless polled"]
pub struct BittorrentHandler {
    /// All currently running torrents in engine.
    torrents: HashMap<TorrentId, TorrentEntry>,
    /// Receiver half for commands.
    command_rx: mpsc::UnboundedReceiver<BttCommand>,
    /// The disk channel.
    disk_tx: disk::Sender,
    disk_join_handle: Option<disk::JoinHandle>,
    /// The channel on which tasks in the engine post alerts to user.
    alert_tx: AlertSender,
    /// The config this client was configured with.
    config: BittorrentConfig,
    /// Counter for torrent ids.
    next_id: u32,
}

impl BittorrentHandler {
    /// Returns the next id for a torrent
    fn next_torrent_id(&mut self) -> TorrentId {
        let id = self.next_id;
        self.next_id += 1;
        TorrentId(id)
    }

    /// Creates and spawns a new torrent based on the parameters given.
    async fn create_torrent(&mut self, id: TorrentId, params: TorrentParams) -> TorrentResult<()> {
        let conf = params.conf.unwrap_or_else(|| self.config.torrent_config.clone());
        let storage_info = StorageInfo::new(&params.metainfo, self.config.download_dir.clone());
        // TODO: don't duplicate trackers if multiple torrents use the same
        // ones (common in practice)
        let trackers = params.metainfo.trackers.into_iter().map(Tracker::new).collect();
        let own_pieces = params.mode.own_pieces(storage_info.piece_count);

        // create and spawn torrent
        let (mut torrent, torrent_tx) = Torrent::new(torrent::Params {
            id,
            disk_tx: self.disk_tx.clone(),
            info_hash: params.metainfo.info_hash,
            storage_info: storage_info.clone(),
            own_pieces,
            trackers,
            client_id: self.config.client_id,
            listen_addr: params
                .listen_addr
                .unwrap_or_else(|| SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0)),
            conf,
            alert_tx: self.alert_tx.clone(),
        });

        // Allocate torrent on disk. This is an asynchronous process and we can
        // start the torrent in the meantime.
        //
        // Technically we could have issues if the torrent connects to peers
        // that send data before we manage to allocate the (empty) files on
        // disk. However, this should be an extremely pathological case for
        // 2 reasons:
        // - Most torrents would be started without peers, so a torrent would have to wait for peers
        //   from its tracker(s). This should be a sufficiently long time to allocate torrent on
        //   disk.
        // - Then, even if we manage to connect peers quickly, testing shows that they don't tend to
        //   unchoke us immediately.
        //
        // Thus there is little chance to receive data and thus cause a disk
        // write or disk read immediately.
        self.disk_tx.send(disk::DiskCommand::NewTorrent {
            id,
            storage_info,
            piece_hashes: params.metainfo.pieces,
            torrent_tx: torrent_tx.clone(),
        })?;

        let seeds = params.mode.seeds();
        let join_handle = task::spawn(async move { torrent.start(&seeds).await });

        self.torrents.insert(id, TorrentEntry { tx: torrent_tx, join_handle: Some(join_handle) });

        Ok(())
    }

    /// Runs the handler until an unrecoverable error occurs, or until the user
    /// sends a shutdown command.
    async fn run(&mut self) -> TorrentResult<()> {
        info!("Starting engine");

        while let Some(cmd) = self.command_rx.recv().await {
            match cmd {
                BttCommand::CreateTorrent { tx, params } => {
                    let id = self.next_torrent_id();
                    self.create_torrent(id, params).await?;
                    tx.send(id);
                }
                BttCommand::TorrentAllocation { id, result } => match result {
                    Ok(_) => {
                        info!("Torrent {} allocated on disk", id);
                    }
                    Err(e) => {
                        error!("Error allocating torrent {} on disk: {}", id, e);
                    }
                },
                BttCommand::Shutdown => {
                    self.shutdown().await?;
                    break
                }
            }
        }

        Ok(())
    }

    /// Gracefully shuts down the engine and all its components.
    async fn shutdown(&mut self) -> TorrentResult<()> {
        info!("Shutting down engine");

        // tell all torrents to shut down and join their tasks
        for torrent in self.torrents.values_mut() {
            // the torrent task may no longer be running, so don't panic here
            torrent.tx.send(torrent::TorrentCommand::Shutdown).ok();
        }
        // Then join all torrent task handles. Shutting down a torrent may take
        // a while, so join as a separate step to first initiate the shutdown of
        // all torrents.
        for torrent in self.torrents.values_mut() {
            if let Err(e) = torrent
                .join_handle
                .take()
                .expect("torrent join handle missing")
                .await
                .expect("task error")
            {
                error!("Torrent error: {}", e);
            }
        }

        // send a shutdown command to disk
        self.disk_tx.send(disk::DiskCommand::Shutdown)?;
        // and join on its handle
        self.disk_join_handle
            .take()
            .expect("disk join handle missing")
            .await
            .expect("Disk task has panicked");

        Ok(())
    }
}

/// All settings used to create a [`Bittorrent`] instance.
#[derive(Debug, Clone, TypedBuilder)]
pub struct BittorrentConfig {
    /// The ID of the client to announce to trackers and other peers.
    #[builder(default=RETH_TORRENT_CLIENT_ID)]
    pub client_id: PeerId,
    /// The directory in which a torrent's files are placed upon download and
    /// from which they are seeded.
    pub download_dir: PathBuf,
    /// Default config for torrents.
    #[builder(default)]
    pub torrent_config: TorrentConfig,
}

// === impl BittorrentConfig ===

impl BittorrentConfig {
    /// Creates a new config with a download dir.
    pub fn new(download_dir: impl Into<PathBuf>) -> Self {
        Self {
            client_id: RETH_TORRENT_CLIENT_ID,
            download_dir: Default::default(),
            torrent_config: Default::default(),
        }
    }
}

/// User facing events.
#[derive(Debug)]
pub enum BittorrentEvent {}

/// Commands that can be sent from [`Bittorrent`] to the [`BittorrentHandler`]
#[allow(clippy::large_enum_variant)]
pub(crate) enum BttCommand {
    /// Contains the information for creating a new torrent.
    CreateTorrent { params: TorrentParams, tx: OneshotSender<TorrentId> },
    /// Torrent allocation result. If successful, the id of the allocated
    /// torrent is returned for identification, if not, the reason of the error
    /// is included.
    TorrentAllocation { id: TorrentId, result: Result<(), NewTorrentError> },
    /// Gracefully shuts down the engine and waits for all its torrents to do
    /// the same.
    Shutdown,
}

/// Information for creating a new torrent.
#[derive(Debug, TypedBuilder)]
pub struct TorrentParams {
    /// Contains the torrent's metadata.
    pub metainfo: Metainfo,
    /// If set, overrides the default global config.
    pub conf: Option<TorrentConfig>,
    /// Whether to download or seed the torrent.
    ///
    /// This is expected to be removed as this will become automatic once
    /// torrent resume data is supported.
    pub mode: Mode,
    /// The address on which the torrent should listen for new peers.
    ///
    /// This has to be unique for each torrent. If not set, or if already in
    /// use, a random port is assigned.
    // TODO: probably use an engine wide address, but requires some
    // rearchitecting
    #[builder(default, setter(strip_option))]
    pub listen_addr: Option<SocketAddr>,
}

/// The download mode.
#[derive(Debug)]
pub enum Mode {
    Download { seeds: Vec<SocketAddr> },
    Seed,
}

impl Mode {
    fn own_pieces(&self, piece_count: usize) -> BitField {
        match self {
            Self::Download { .. } => BitField::new_all_clear(piece_count),
            Self::Seed => BitField::new_all_set(piece_count),
        }
    }

    fn seeds(self) -> Vec<SocketAddr> {
        match self {
            Self::Download { seeds } => seeds,
            _ => Vec::new(),
        }
    }
}

/// A running torrent's entry in the engine.
struct TorrentEntry {
    /// The torrent's command channel on which engine sends commands to torrent.
    tx: torrent::Sender,
    /// The torrent task's join handle, used during shutdown.
    join_handle: Option<task::JoinHandle<TorrentResult<()>>>,
}
