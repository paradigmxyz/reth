use crate::{
    error::TorrentResult,
    info::Metainfo,
    sha1::{PeerId, RETH_TORRENT_CLIENT_ID},
    torrent::{config::TorrentConfig, pool::TorrentPool, TorrentId},
};
use futures::Stream;
use std::{
    net::SocketAddr,
    path::PathBuf,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::{
    mpsc, oneshot,
    oneshot::{channel, Sender as OneshotSender},
};
use tracing::trace;
use typed_builder::TypedBuilder;
use crate::disk::error::NewTorrentError;

/// Provides the API to interact with torrents.
#[derive(Debug, Clone)]
pub struct Bittorrent {
    /// Channel used to communicate with the [`BittorrentHandler`].
    command_tx: mpsc::UnboundedSender<BttCommand>,
}

// === impl Bittorrent ===

impl Bittorrent {
    /// Creates a new instance of [`Bittorrent`] and the [`BittorrentHandler`] task.
    pub fn new(_config: BittorrentConfig) -> (Self, BittorrentHandler) {
        todo!()
    }

    /// Creates and starts a torrent, if its metainfo is valid.
    ///
    /// If successful, it returns the id of the torrent. This id can be used to
    /// identify the torrent when issuing further commands to engine.
    pub async fn create_torrent(&self, params: TorrentParams) -> TorrentResult<TorrentId> {
        let (tx, rx) = oneshot::channel();
        self.command_tx.send(BttCommand::CreateTorrent { params, tx })?;
        rx.await?
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
    /// All active torrents.
    pool: TorrentPool,
    /// Receiver half for commands.
    command_rx: mpsc::UnboundedReceiver<BttCommand>,
    /// The config this client was configured with.
    config: BittorrentConfig,
}

impl Stream for BittorrentHandler {
    type Item = BittorrentEvent;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        todo!()
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
    CreateTorrent { params: TorrentParams, tx: OneshotSender<TorrentResult<TorrentId>> },
    /// Torrent allocation result. If successful, the id of the allocated
    /// torrent is returned for identification, if not, the reason of the error
    /// is included.
    TorrentAllocation {
        id: TorrentId,
        result: Result<(), NewTorrentError>,
    },
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
