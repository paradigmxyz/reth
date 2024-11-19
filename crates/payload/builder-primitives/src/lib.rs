//! This crate defines abstractions to create and update payloads (blocks)

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

mod events;
pub use crate::events::{Events, PayloadEvents};

/// Contains the payload builder trait to abstract over payload attributes.
mod traits;
pub use traits::{PayloadBuilder, PayloadStoreExt};

pub use reth_payload_primitives::PayloadBuilderError;
