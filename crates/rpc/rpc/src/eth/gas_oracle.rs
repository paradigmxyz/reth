//! An implementation of the eth gas price oracle, used for providing gas price estimates based on
//! previous blocks.
use futures::StreamExt;
use reth_interfaces::Result;
use reth_primitives::{Block, H256, U256};
use reth_provider::BlockProvider;
use reth_tasks::{TaskSpawner, TokioTaskExecutor};
use serde::{Deserialize, Serialize};
use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio_stream::wrappers::UnboundedReceiverStream;

// go code to port to rust
// const sampleNumber = 3 // Number of transactions sampled in a block

// var (
// 	DefaultMaxPrice    = big.NewInt(500 * params.GWei)
// 	DefaultIgnorePrice = big.NewInt(2 * params.Wei)
// )

// type Config struct {
// 	Blocks           int
// 	Percentile       int
// 	MaxHeaderHistory uint64
// 	MaxBlockHistory  uint64
// 	// starting latest price
// 	Default          *big.Int `toml:",omitempty"`
// 	MaxPrice         *big.Int `toml:",omitempty"`
// 	IgnorePrice      *big.Int `toml:",omitempty"`
// }

/// The number of transactions sampled in a block
pub const SAMPLE_NUMBER: u32 = 3;

/// The default maximum gas price to use for the estimate
pub const DEFAULT_MAX_PRICE: U256 = U256::from_limbs([500_000_000_000u64, 0, 0, 0]);

/// The default minimum gas price, under which the sample will be ignored
pub const DEFAULT_IGNORE_PRICE: U256 = U256::from_limbs([2u64, 0, 0, 0]);

/// Settings for the [GasPriceOracle]
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GasOracleConfig {
    /// The number of blocks to sample for gas price estimates
    pub blocks: u32,

    /// The percentile of gas prices to use for the estimate
    pub percentile: u32,

    /// The maximum number of headers to keep in the cache
    pub max_header_history: u64,

    /// The maximum number of blocks to keep in the cache
    pub max_block_history: u64,

    /// The default gas price to use if there are no blocks in the cache
    pub default: U256,

    /// The maximum gas price to use for the estimate
    pub max_price: U256,

    /// The minimum gas price, under which the sample will be ignored
    pub ignore_price: U256,
}

// the go code to port to rust
// // Oracle recommends gas prices based on the content of recent
// // blocks. Suitable for both light and full clients.
// type Oracle struct {
// 	backend     OracleBackend
// 	lastHead    common.Hash
// 	lastPrice   *big.Int
// 	maxPrice    *big.Int
// 	ignorePrice *big.Int
// 	cacheLock   sync.RWMutex
// 	fetchLock   sync.Mutex

// 	checkBlocks, percentile           int
// 	maxHeaderHistory, maxBlockHistory uint64

// 	historyCache *lru.Cache[cacheKey, processedFees]
// }

/// Provides async access to the gas price oracle.
///
/// This is the frontend for the async gas price oracle which manages gas price estimates on a
/// different task.
#[derive(Debug, Clone)]
pub struct GasPriceOracle {
    to_service: UnboundedSender<GasPriceAction>,
}

/// All message variants sent to the oracle through the channel
#[derive(Debug, Clone)]
pub enum GasPriceAction {
    // TODO
}

impl GasPriceOracle {
    /// Creates and returns the [GasPriceOracle] frontend.
    fn create<Client, Tasks>(
        client: Client,
        action_task_spawner: Tasks,
        oracle_config: GasOracleConfig,
    ) -> (Self, GasPriceOracleService<Client, Tasks>) {
        let (to_service, rx) = unbounded_channel();
        let service = GasPriceOracleService {
            client,
            oracle_config,
            action_tx: to_service.clone(),
            action_rx: UnboundedReceiverStream::new(rx),
            action_task_spawner,
        };
        let oracle = Self { to_service };
        (oracle, service)
    }

    /// Creates a new async gas oracle service task and spawns it to a new task via [tokio::spawn].
    ///
    /// See also [Self::spawn_with]
    pub fn spawn<Client>(client: Client, config: GasOracleConfig) -> Self
    where
        Client: BlockProvider + Clone + Unpin + 'static,
    {
        Self::spawn_with(client, config, TokioTaskExecutor::default())
    }

    /// Creates a new async gas oracle service task and spawns it to a new task via the given
    /// spawner.
    pub fn spawn_with<Client, Tasks>(
        client: Client,
        config: GasOracleConfig,
        executor: Tasks,
    ) -> Self
    where
        Client: BlockProvider + Clone + Unpin + 'static,
        Tasks: TaskSpawner + Clone + 'static,
    {
        let (this, service) = Self::create(client, executor.clone(), config);
        executor.spawn_critical("eth gas price oracle", Box::pin(service));
        this
    }
}

/// A task that manages gas price estimates.
///
/// Caution: The channel for the data is _unbounded_ it is assumed that this is mainly used by the
/// [EthApi](crate::EthApi) which is typically invoked by the RPC server, which already uses permits
/// to limit concurrent requests.
#[must_use = "Type does nothing unless spawned"]
pub(crate) struct GasPriceOracleService<Client, Tasks> {
    /// The type used to subscribe to block events and get block info
    client: Client,
    /// The config for the oracle
    oracle_config: GasOracleConfig,
    /// Sender half of the action channel.
    action_tx: UnboundedSender<GasPriceAction>,
    /// Receiver half of the action channel.
    action_rx: UnboundedReceiverStream<GasPriceAction>,
    /// The type that's used to spawn tasks that do the actual work
    action_task_spawner: Tasks,
}

impl<Client, Tasks> GasPriceOracleService<Client, Tasks>
where
    Client: BlockProvider + Clone + Unpin + 'static,
    Tasks: TaskSpawner + Clone + 'static,
{
    fn on_new_block(&mut self, block_hash: H256, res: Result<Option<Block>>) {
        todo!()
    }
}

impl<Client, Tasks> Future for GasPriceOracleService<Client, Tasks>
where
    Client: BlockProvider + Clone + Unpin + 'static,
    Tasks: TaskSpawner + Clone + 'static,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            match ready!(this.action_rx.poll_next_unpin(cx)) {
                None => {
                    unreachable!("can't close")
                }
                Some(action) => match action {},
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use reth_primitives::constants::GWEI_TO_WEI;

    use super::*;

    #[test]
    fn max_price_sanity() {
        assert_eq!(DEFAULT_MAX_PRICE, U256::from(500_000_000_000u64));
        assert_eq!(DEFAULT_MAX_PRICE, U256::from(500 * GWEI_TO_WEI))
    }

    #[test]
    fn ignore_price_sanity() {
        assert_eq!(DEFAULT_IGNORE_PRICE, U256::from(2u64));
    }
}
