//! Scroll engine API crate.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars.githubusercontent.com/u/87750292?s=200&v=4",
    issue_tracker_base_url = "https://github.com/scroll-tech/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg(all(feature = "scroll", not(feature = "optimism")))]

extern crate alloc;

mod payload;
pub use payload::utils::try_into_block;
