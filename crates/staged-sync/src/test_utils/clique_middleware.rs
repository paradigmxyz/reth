//! Helper extension traits for working with clique providers.

use async_trait::async_trait;
use enr::k256::ecdsa::SigningKey;
use ethers_core::{
    types::{transaction::eip2718::TypedTransaction, Block, BlockNumber, H256},
    utils::secret_key_to_address,
};
use ethers_middleware::SignerMiddleware;
use ethers_providers::Middleware;
use ethers_signers::Signer;
use reth_network::test_utils::enr_to_peer_id;
use reth_primitives::PeerId;
use thiserror::Error;
use tracing::trace;

/// An error that can occur when using the [`CliqueMiddleware`].
#[derive(Error, Debug)]
pub enum CliqueError<E> {
    /// Error encountered when using the provider
    #[error(transparent)]
    MiddlewareError(#[from] E),

    /// No genesis block returned from the provider
    #[error("no genesis block returned from the provider")]
    NoGenesis,

    /// No tip block returned from the provider
    #[error("no tip block returned from the provider")]
    NoTip,
}

/// Error type for [`CliqueMiddleware`].
pub type CliqueMiddlewareError<M> = CliqueError<<M as Middleware>::Error>;

/// Extension trait for [`Middleware`] to provide clique specific functionality.
#[async_trait(?Send)]
pub trait CliqueMiddleware: Send + Sync + Middleware {
    /// Enable mining on the clique geth instance by importing and unlocking the signer account
    /// derived from given private key and password.
    async fn enable_mining(&self, signer: SigningKey, password: String) {
        let our_address = secret_key_to_address(&signer);

        // send the private key to geth and unlock it
        let key_bytes = signer.to_bytes().to_vec().into();
        trace!(
            private_key=%hex::encode(&key_bytes),
            "Importing private key"
        );

        let unlocked_addr = self.import_raw_key(key_bytes, password.to_string()).await.unwrap();
        assert_eq!(unlocked_addr, our_address);

        let unlock_success =
            self.unlock_account(our_address, password.to_string(), None).await.unwrap();
        assert!(unlock_success);

        // start mining?
        self.start_mining(None).await.unwrap();

        // check that we are mining
        let mining = self.mining().await.unwrap();
        assert!(mining);
    }

    /// Returns the chain tip of the [`Geth`](ethers_core::utils::Geth) instance by calling
    /// geth's `eth_getBlock`.
    async fn remote_tip_block(&self) -> Result<Block<H256>, CliqueMiddlewareError<Self>> {
        self.get_block(BlockNumber::Latest).await?.ok_or(CliqueError::NoTip)
    }

    /// Returns the genesis block of the [`Geth`](ethers_core::utils::Geth) instance by calling
    /// geth's `eth_getBlock`.
    async fn remote_genesis_block(&self) -> Result<Block<H256>, CliqueMiddlewareError<Self>> {
        self.get_block(BlockNumber::Earliest).await?.ok_or(CliqueError::NoGenesis)
    }

    /// Signs and sends the given unsigned transactions sequentially, signing with the private key
    /// used to configure the [`CliqueGethInstance`].
    async fn send_requests<T: IntoIterator<Item = TypedTransaction>>(
        &self,
        txs: T,
    ) -> Result<(), CliqueMiddlewareError<Self>> {
        for tx in txs {
            self.send_transaction(tx, None).await?;
        }
        Ok(())
    }

    /// Returns the [`Geth`](ethers_core::utils::Geth) instance [`PeerId`](reth_primitives::PeerId)
    /// by calling geth's `admin_nodeInfo`.
    async fn peer_id(&self) -> Result<PeerId, CliqueMiddlewareError<Self>> {
        Ok(enr_to_peer_id(self.node_info().await?.enr))
    }
}

impl<M: Middleware, S: Signer> CliqueMiddleware for SignerMiddleware<M, S> {}
