use std::time::Duration;

/// Configuration for a torrent.
///
/// The engine will have a default instance of this applied to all torrents by
/// default, but individual torrents may override this configuration.
#[derive(Clone, Debug)]
pub struct TorrentConfig {
    /// The minimum number of peers we want to keep in torrent at all times.
    /// This will be configurable later.
    pub min_requested_peer_count: usize,
    /// The max number of connected peers the torrent should have.
    pub max_connected_peer_count: usize,
    /// If the tracker doesn't provide a minimum announce interval, we default
    /// to announcing every 30 seconds.
    pub announce_interval: Duration,
    /// After this many attempts, the torrent stops announcing to a tracker.
    pub tracker_error_threshold: usize,
    /// Specifies which optional alerts to send, besides the default periodic
    /// stats update.
    pub alerts: TorrentAlertConf,
}

/// Configuration of a torrent's optional alerts.
///
/// By default, all optional alerts are turned off. This is because some of
/// these alerts may have overhead that shouldn't be paid when the alerts are
/// not used.
#[derive(Clone, Debug, Default)]
pub struct TorrentAlertConf {
    /// Receive the pieces that were completed each round.
    ///
    /// This has minor overhead and so it may be enabled. For full optimization,
    /// however, it is only enabled when either the pieces or individual file
    /// completions are needed.
    pub completed_pieces: bool,
    /// Receive aggregate statistics about the torrent's peers.
    ///
    /// This may be relatively expensive. It is suggested to only turn it on
    /// when it is specifically needed, e.g. when the UI is showing the peers of
    /// a torrent.
    pub peers: bool,
}

impl Default for TorrentConfig {
    fn default() -> Self {
        Self {
            // We always request at least 10 peers as anything less is a waste
            // of network round trip and it allows us to buffer up a bit more
            // than needed.
            min_requested_peer_count: 10,
            // This value is mostly picked for performance while keeping in mind
            // not to overwhelm the host.
            max_connected_peer_count: 50,
            // needs teting
            announce_interval: Duration::from_secs(60 * 60),
            // needs testing
            tracker_error_threshold: 15,
            alerts: Default::default(),
        }
    }
}
