//! Standalone crate for Optimism-codecs for Reth storage.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

// The `optimism` feature must be enabled to use this module.
#[cfg(feature = "optimism")]
pub mod tx_deposit;
// The `optimism` feature must be enabled to use this module.
#[cfg(feature = "optimism")]
pub use tx_deposit::{TxDepositDecode, TxDepositEncode};

#[cfg(test)]
// The `optimism` feature must be enabled to use this module.
#[cfg(feature = "optimism")]
mod tests {
    use reth_codecs::{test_utils::UnusedBits, validate_bitflag_backwards_compat};
    use reth_primitives::Receipt;

    #[test]
    fn test_ensure_backwards_compatibility() {
        assert_eq!(Receipt::bitflag_encoded_bytes(), 2);

        // In case of failure, refer to the documentation of the
        // [`validate_bitflag_backwards_compat`] macro for detailed instructions on handling
        // it.
        validate_bitflag_backwards_compat!(Receipt, UnusedBits::NotZero);
    }
}
