//! Contains RPC handler implementations specific to sign endpoints

use crate::eth::{signer::DevSigner, EthApi};

impl<Provider, Pool, Network, EvmConfig> EthApi<Provider, Pool, Network, EvmConfig> {
    /// Generates 20 random developer accounts.
    /// Used in DEV mode.
    pub fn with_dev_accounts(&self) {
        let mut signers = self.inner.signers.write();
        *signers = DevSigner::random_signers(20);
    }
}
