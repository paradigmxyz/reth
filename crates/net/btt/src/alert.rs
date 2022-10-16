//! This module defines the alerts the API user may receive from the torrent
//! engine.
//!
//! Communication of such alerts is performed via unbounded [tokio mpsc
//! channels](tokio::sync::mpsc). Thus, the application in which the engine is
//! integrated may be driven partially or entirely by cratetorrent alerts.
//!
//! # Optional information
//!
//! By default only the most basic alerts are broadcast from the engine. The
//! reason for this is that cratetorrent follows a philosophy similar what lies
//! behind Rust or C++: pay for only what you use.
//!
//! This is of course not fully possible with something as complex as a torrent
//! engine, but an effort is made to make more expensive operations optional.
//!
//! Such alerts include the [latest downloaded
//! pieces](crate::conf::TorrentAlertConf::completed_pieces) or aggregate
//! statistics about a torrent's [peers](crate::conf::TorrentAlertConf::peers).
//! More will be added later.

use crate::{
    error::Error,
    torrent::{stats::TorrentStats, TorrentId},
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_stream::wrappers::UnboundedReceiverStream;

pub(crate) type AlertSender = UnboundedSender<Alert>;
/// The channel on which alerts from the engine can be received. See [`Alert`]
/// for the type of messages that can be received.
pub type AlertReceiver = UnboundedReceiver<Alert>;

/// A Stream that yields `Alerts`
pub type AlertStream = UnboundedReceiverStream<Alert>;

/// The alerts that the engine may send the library user.
#[derive(Debug)]
#[non_exhaustive]
pub enum Alert {
    /// Posted when the torrent has finished downloading.
    TorrentComplete(TorrentId),
    /// Each running torrent sends an update of its latest statistics every
    /// second via this alert.
    TorrentStats { id: TorrentId, stats: Box<TorrentStats> },
    /// An error from somewhere inside the engine.
    Error(Error),
}
