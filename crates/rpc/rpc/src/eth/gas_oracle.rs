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
use tokio::sync::{
    mpsc::{unbounded_channel, UnboundedSender},
    oneshot,
};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// The number of transactions sampled in a block
pub const SAMPLE_NUMBER: u32 = 3;

/// The default maximum gas price to use for the estimate
pub const DEFAULT_MAX_PRICE: U256 = U256::from_limbs([500_000_000_000u64, 0, 0, 0]);

/// The default minimum gas price, under which the sample will be ignored
pub const DEFAULT_IGNORE_PRICE: U256 = U256::from_limbs([2u64, 0, 0, 0]);

/// The type that can send the response to the requested max priority fee.
type MaxPriorityFeeSender = oneshot::Sender<Result<U256>>;

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
    pub default: Option<U256>,

    /// The maximum gas price to use for the estimate
    pub max_price: Option<U256>,

    /// The minimum gas price, under which the sample will be ignored
    pub ignore_price: Option<U256>,
}

// TODO: other config defaults

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
pub enum GasPriceAction {
    SuggestMaxPriorityFee { response_tx: MaxPriorityFeeSender },
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

    //// SuggestTipCap returns a tip cap so that newly created transaction can have a
    //// very high chance to be included in the following blocks.
    ////
    //// Note, for legacy transactions and the legacy eth_gasPrice RPC call, it will be
    //// necessary to add the basefee to the returned number to fall back to the legacy
    //// behavior.
    //func (oracle *Oracle) SuggestTipCap(ctx context.Context) (*big.Int, error) {
    fn suggest_tip_cap(&self) -> Result<U256> {
        todo!()
    }

    // go code to port to rust
    // // getBlockValues calculates the lowest transaction gas price in a given block
    // // and sends it to the result channel. If the block is empty or all transactions
    // // are sent by the miner itself(it doesn't make any sense to include this kind of
    // // transaction prices for sampling), nil gasprice is returned.
    // func (oracle *Oracle) getBlockValues(ctx context.Context, blockNum uint64, limit int,
    // ignoreUnder *big.Int, result chan results, quit chan struct{}) { 	block, err :=
    // oracle.backend.BlockByNumber(ctx, rpc.BlockNumber(blockNum)) 	if block == nil {
    // 		select {
    // 		case result <- results{nil, err}:
    // 		case <-quit:
    // 		}
    // 		return
    // 	}
    // 	signer := types.MakeSigner(oracle.backend.ChainConfig(), block.Number(), block.Time())

    // 	// Sort the transaction by effective tip in ascending sort.
    // 	txs := make([]*types.Transaction, len(block.Transactions()))
    // 	copy(txs, block.Transactions())
    // 	sorter := newSorter(txs, block.BaseFee())
    // 	sort.Sort(sorter)

    // 	var prices []*big.Int
    // 	for _, tx := range sorter.txs {
    // 		tip, _ := tx.EffectiveGasTip(block.BaseFee())
    // 		if ignoreUnder != nil && tip.Cmp(ignoreUnder) == -1 {
    // 			continue
    // 		}
    // 		sender, err := types.Sender(signer, tx)
    // 		if err == nil && sender != block.Coinbase() {
    // 			prices = append(prices, tip)
    // 			if len(prices) >= limit {
    // 				break
    // 			}
    // 		}
    // 	}
    // 	select {
    // 	case result <- results{prices, nil}:
    // 	case <-quit:
    // 	}
    // }
    fn get_block_values(&self, block_num: u64, limit: usize) -> Result<Option<Vec<Option<U256>>>> {
        let block = match self.client.block_by_number(block_num)? {
            Some(block) => block,
            None => return Ok(None),
        };

        // sort the transactions by effective tip
        // but first filter those that should be ignored
        let txs = block.body.iter();
        let mut txs = txs
            .filter(|tx| {
                if let Some(ignore_under) = self.oracle_config.ignore_price {
                    if tx.effective_gas_tip(block.base_fee_per_gas).map(U256::from) <
                        Some(ignore_under)
                    {
                        return true
                    }
                }

                // recover sender, check if coinbase
                let sender = tx.recover_signer();
                match sender {
                    Some(addr) => addr == block.beneficiary,
                    // invalid signature - should not happen, but ignore?
                    // TODO: figure out this case
                    None => true,
                }
            })
            .collect::<Vec<_>>();

        // now do the sort
        // TODO: could we cache tx effective_gas_tip? might be a lot of complexity for little use
        txs.sort_by(|first, second| {
            first
                .effective_gas_tip(block.base_fee_per_gas)
                .cmp(&second.effective_gas_tip(block.base_fee_per_gas))
        });

        Ok(Some(
            txs[..limit]
                .iter()
                .map(|tx| tx.effective_gas_tip(block.base_fee_per_gas).map(U256::from))
                .collect(),
        ))
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
                Some(action) => match action {
                    GasPriceAction::SuggestMaxPriorityFee { response_tx } => {
                        todo!()
                        // let _ = response_tx.send(result);
                    }
                },
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
