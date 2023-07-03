//! Helper struct for working with a clique geth instance.
use enr::k256::ecdsa::SigningKey;
use ethers_core::utils::{Geth, GethInstance};
use ethers_middleware::SignerMiddleware;
use ethers_providers::{Provider, Ws};
use ethers_signers::{LocalWallet, Wallet};
use std::{
    io::{BufRead, BufReader},
    net::SocketAddr,
};

/// A [`Geth`](ethers_core::utils::Geth) instance configured with Clique and a custom
/// [`Genesis`](ethers_core::utils::Genesis).
///
/// This holds a [`SignerMiddleware`](ethers_middleware::SignerMiddleware) for
/// enabling block production and creating transactions.
///
/// # Example
/// ```no_run
/// # use ethers_core::utils::Geth;
/// # use reth_staged_sync::test_utils::CliqueGethInstance;
/// # let clique = async {
///
/// // this creates a funded geth
/// let clique_geth = Geth::new()
///     .p2p_port(30303)
///     .chain_id(1337u64);
///
/// // build the funded geth, generating a random signing key and enabling clique
/// let mut clique = CliqueGethInstance::new(clique_geth, None).await;
///
/// // don't print logs, but drain the stderr
/// clique.prevent_blocking().await;
/// # };
/// ```
pub struct CliqueGethInstance {
    /// The spawned [`GethInstance`](ethers_core::utils::GethInstance).
    pub instance: GethInstance,
    /// The provider who can talk to this instance
    pub provider: SignerMiddleware<Provider<Ws>, Wallet<SigningKey>>,
}

impl CliqueGethInstance {
    /// Sets up a new [`SignerMiddleware`](ethers_middleware::SignerMiddleware)
    /// for the [`Geth`](ethers_core::utils::Geth) instance and returns the
    /// [`CliqueGethInstance`](crate::test_utils::CliqueGethInstance).
    ///
    /// The signer is assumed to be the clique signer and the signer for any transactions sent for
    /// block production.
    ///
    /// This also spawns the geth instance.
    pub async fn new(geth: Geth, signer: Option<SigningKey>) -> Self {
        let signer = signer.unwrap_or_else(|| SigningKey::random(&mut rand::thread_rng()));

        let geth = geth.set_clique_private_key(signer.clone());

        // spawn the geth instance
        let instance = geth.spawn();

        // create the signer
        let wallet: LocalWallet = signer.clone().into();

        // set up ethers provider
        let geth_endpoint = SocketAddr::new([127, 0, 0, 1].into(), instance.port()).to_string();
        let provider = Provider::<Ws>::connect(format!("ws://{geth_endpoint}")).await.unwrap();
        let provider =
            SignerMiddleware::new_with_provider_chain(provider, wallet.clone()).await.unwrap();

        Self { instance, provider }
    }

    /// Prints the logs of the [`Geth`](ethers_core::utils::Geth) instance in a new
    /// [`task`](tokio::task).
    #[allow(dead_code)]
    pub async fn print_logs(&mut self) {
        // take the stderr of the geth instance and print it
        let stderr = self.instance.stderr().unwrap();

        // print logs in a new task
        let mut err_reader = BufReader::new(stderr);

        tokio::spawn(async move {
            loop {
                if let (Ok(line), line_str) = {
                    let mut buf = String::new();
                    (err_reader.read_line(&mut buf), buf.clone())
                } {
                    if line == 0 {
                        break
                    }
                    if !line_str.is_empty() {
                        dbg!(line_str);
                    }
                }
            }
        });
    }

    /// Prevents the [`Geth`](ethers_core::utils::Geth) instance from blocking due to the `stderr`
    /// filling up.
    pub async fn prevent_blocking(&mut self) {
        // take the stderr of the geth instance and print it
        let stderr = self.instance.stderr().unwrap();

        // print logs in a new task
        let mut err_reader = BufReader::new(stderr);

        tokio::spawn(async move {
            loop {
                if let (Ok(line), _line_str) = {
                    let mut buf = String::new();
                    (err_reader.read_line(&mut buf), buf.clone())
                } {
                    if line == 0 {
                        break
                    }
                }
            }
        });
    }
}
