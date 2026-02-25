//! Test helpers for mocking an entire pool.

#![allow(dead_code)]

use crate::{
    error::PoolErrorKind,
    pool::{state::SubPool, txpool::TxPool, AddedTransaction},
    test_utils::{MockOrdering, MockTransactionDistribution, MockTransactionFactory},
    TransactionOrdering,
};
use alloy_primitives::{map::AddressMap, Address, U256};
use rand::Rng;
use std::ops::{Deref, DerefMut};

/// A wrapped `TxPool` with additional helpers for testing
pub(crate) struct MockPool<T: TransactionOrdering = MockOrdering> {
    // The wrapped pool.
    pool: TxPool<T>,
}

impl MockPool {
    /// The total size of all subpools
    fn total_subpool_size(&self) -> usize {
        self.pool.pending().len() + self.pool.base_fee().len() + self.pool.queued().len()
    }

    /// Checks that all pool invariants hold.
    fn enforce_invariants(&self) {
        assert_eq!(
            self.pool.len(),
            self.total_subpool_size(),
            "Tx in AllTransactions and sum(subpools) must match"
        );
    }
}

impl Default for MockPool {
    fn default() -> Self {
        Self { pool: TxPool::new(MockOrdering::default(), Default::default()) }
    }
}

impl<T: TransactionOrdering> Deref for MockPool<T> {
    type Target = TxPool<T>;

    fn deref(&self) -> &Self::Target {
        &self.pool
    }
}

impl<T: TransactionOrdering> DerefMut for MockPool<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.pool
    }
}

/// Simulates transaction execution.
pub(crate) struct MockTransactionSimulator<R: Rng> {
    /// The pending base fee
    base_fee: u128,
    /// Generator for transactions
    tx_generator: MockTransactionDistribution,
    /// represents the on chain balance of a sender.
    balances: AddressMap<U256>,
    /// represents the on chain nonce of a sender.
    nonces: AddressMap<u64>,
    /// A set of addresses to use as senders.
    senders: Vec<Address>,
    /// What scenarios to execute.
    scenarios: Vec<ScenarioType>,
    /// All previous scenarios executed by a sender.
    executed: AddressMap<ExecutedScenarios>,
    /// "Validates" generated transactions.
    validator: MockTransactionFactory,
    /// Represents the gaps in nonces for each sender.
    nonce_gaps: AddressMap<u64>,
    /// The rng instance used to select senders and scenarios.
    rng: R,
}

impl<R: Rng> MockTransactionSimulator<R> {
    /// Returns a new mock instance
    pub(crate) fn new(mut rng: R, config: MockSimulatorConfig) -> Self {
        let senders = config.addresses(&mut rng);
        Self {
            base_fee: config.base_fee,
            balances: senders.iter().copied().map(|a| (a, rng.random())).collect(),
            nonces: senders.iter().copied().map(|a| (a, 0)).collect(),
            senders,
            scenarios: config.scenarios,
            tx_generator: config.tx_generator,
            executed: Default::default(),
            validator: Default::default(),
            nonce_gaps: Default::default(),
            rng,
        }
    }

    /// Creates a pool configured for this simulator
    ///
    /// This is needed because `MockPool::default()` sets `pending_basefee` to 7, but we might want
    /// to use different values
    pub(crate) fn create_pool(&self) -> MockPool {
        let mut pool = MockPool::default();
        let mut info = pool.block_info();
        info.pending_basefee = self.base_fee as u64;
        pool.set_block_info(info);
        pool
    }

    /// Returns a random address from the senders set
    fn rng_address(&mut self) -> Address {
        let idx = self.rng.random_range(0..self.senders.len());
        self.senders[idx]
    }

    /// Returns a random scenario from the scenario set
    fn rng_scenario(&mut self) -> ScenarioType {
        let idx = self.rng.random_range(0..self.scenarios.len());
        self.scenarios[idx].clone()
    }

    /// Executes the next scenario and applies it to the pool
    pub(crate) fn next(&mut self, pool: &mut MockPool) {
        let sender = self.rng_address();
        let scenario = self.rng_scenario();
        let on_chain_nonce = self.nonces[&sender];
        let on_chain_balance = self.balances[&sender];

        match scenario {
            ScenarioType::OnchainNonce => {
                // uses fee from fee_ranges
                let tx = self.tx_generator.tx(on_chain_nonce, &mut self.rng).with_sender(sender);
                let valid_tx = self.validator.validated(tx);

                let res =
                    match pool.add_transaction(valid_tx, on_chain_balance, on_chain_nonce, None) {
                        Ok(res) => res,
                        Err(e) => match e.kind {
                            // skip pool capacity/replacement errors (not relevant)
                            PoolErrorKind::SpammerExceededCapacity(_) |
                            PoolErrorKind::ReplacementUnderpriced => return,
                            _ => panic!("unexpected error: {e:?}"),
                        },
                    };

                match res {
                    AddedTransaction::Pending(_) => {}
                    AddedTransaction::Parked { .. } => {
                        panic!("expected pending")
                    }
                }

                self.executed
                    .entry(sender)
                    .or_insert_with(|| ExecutedScenarios { sender, scenarios: vec![] }) // in the case of a new sender
                    .scenarios
                    .push(ExecutedScenario {
                        balance: on_chain_balance,
                        nonce: on_chain_nonce,
                        scenario: Scenario::OnchainNonce { nonce: on_chain_nonce },
                    });

                self.nonces.insert(sender, on_chain_nonce + 1);
            }

            ScenarioType::HigherNonce { skip } => {
                // if this sender already has a nonce gap, skip
                if self.nonce_gaps.contains_key(&sender) {
                    return;
                }

                let higher_nonce = on_chain_nonce + skip;

                // uses fee from fee_ranges
                let tx = self.tx_generator.tx(higher_nonce, &mut self.rng).with_sender(sender);
                let valid_tx = self.validator.validated(tx);

                let res =
                    match pool.add_transaction(valid_tx, on_chain_balance, on_chain_nonce, None) {
                        Ok(res) => res,
                        Err(e) => match e.kind {
                            // skip pool capacity/replacement errors (not relevant)
                            PoolErrorKind::SpammerExceededCapacity(_) |
                            PoolErrorKind::ReplacementUnderpriced => return,
                            _ => panic!("unexpected error: {e:?}"),
                        },
                    };

                match res {
                    AddedTransaction::Pending(_) => {
                        panic!("expected parked")
                    }
                    AddedTransaction::Parked { subpool, .. } => {
                        assert_eq!(
                            subpool,
                            SubPool::Queued,
                            "expected to be moved to queued subpool"
                        );
                    }
                }

                self.executed
                    .entry(sender)
                    .or_insert_with(|| ExecutedScenarios { sender, scenarios: vec![] }) // in the case of a new sender
                    .scenarios
                    .push(ExecutedScenario {
                        balance: on_chain_balance,
                        nonce: on_chain_nonce,
                        scenario: Scenario::HigherNonce {
                            onchain: on_chain_nonce,
                            nonce: higher_nonce,
                        },
                    });
                self.nonce_gaps.insert(sender, higher_nonce);
            }

            ScenarioType::BelowBaseFee { fee } => {
                // fee should be in [MIN_PROTOCOL_BASE_FEE, base_fee)
                let tx = self
                    .tx_generator
                    .tx(on_chain_nonce, &mut self.rng)
                    .with_sender(sender)
                    .with_gas_price(fee);
                let valid_tx = self.validator.validated(tx);

                let res =
                    match pool.add_transaction(valid_tx, on_chain_balance, on_chain_nonce, None) {
                        Ok(res) => res,
                        Err(e) => match e.kind {
                            // skip pool capacity/replacement errors (not relevant)
                            PoolErrorKind::SpammerExceededCapacity(_) |
                            PoolErrorKind::ReplacementUnderpriced => return,
                            _ => panic!("unexpected error: {e:?}"),
                        },
                    };

                match res {
                    AddedTransaction::Pending(_) => panic!("expected parked"),
                    AddedTransaction::Parked { subpool, .. } => {
                        assert_eq!(
                            subpool,
                            SubPool::BaseFee,
                            "expected to be moved to base fee subpool"
                        );
                    }
                }
                self.executed
                    .entry(sender)
                    .or_insert_with(|| ExecutedScenarios { sender, scenarios: vec![] }) // in the case of a new sender
                    .scenarios
                    .push(ExecutedScenario {
                        balance: on_chain_balance,
                        nonce: on_chain_nonce,
                        scenario: Scenario::BelowBaseFee { fee },
                    });
            }

            ScenarioType::FillNonceGap => {
                if self.nonce_gaps.is_empty() {
                    return;
                }

                let gap_senders: Vec<Address> = self.nonce_gaps.keys().copied().collect();
                let idx = self.rng.random_range(0..gap_senders.len());
                let gap_sender = gap_senders[idx];
                let queued_nonce = self.nonce_gaps[&gap_sender];

                let sender_onchain_nonce = self.nonces[&gap_sender];
                let sender_balance = self.balances[&gap_sender];

                for fill_nonce in sender_onchain_nonce..queued_nonce {
                    let tx =
                        self.tx_generator.tx(fill_nonce, &mut self.rng).with_sender(gap_sender);
                    let valid_tx = self.validator.validated(tx);

                    let res = match pool.add_transaction(
                        valid_tx,
                        sender_balance,
                        sender_onchain_nonce,
                        None,
                    ) {
                        Ok(res) => res,
                        Err(e) => match e.kind {
                            // skip pool capacity/replacement errors (not relevant)
                            PoolErrorKind::SpammerExceededCapacity(_) |
                            PoolErrorKind::ReplacementUnderpriced => return,
                            _ => panic!("unexpected error: {e:?}"),
                        },
                    };

                    match res {
                        AddedTransaction::Pending(_) => {}
                        AddedTransaction::Parked { .. } => {
                            panic!("expected pending when filling gap")
                        }
                    }

                    self.executed
                        .entry(gap_sender)
                        .or_insert_with(|| ExecutedScenarios {
                            sender: gap_sender,
                            scenarios: vec![],
                        })
                        .scenarios
                        .push(ExecutedScenario {
                            balance: sender_balance,
                            nonce: fill_nonce,
                            scenario: Scenario::FillNonceGap {
                                filled_nonce: fill_nonce,
                                promoted_nonce: queued_nonce,
                            },
                        });
                }
                self.nonces.insert(gap_sender, queued_nonce + 1);
                self.nonce_gaps.remove(&gap_sender);
            }
        }
        // make sure everything is set
        pool.enforce_invariants();
    }
}

/// How to configure a new mock transaction stream
pub(crate) struct MockSimulatorConfig {
    /// How many senders to generate.
    pub(crate) num_senders: usize,
    /// Scenarios to test
    pub(crate) scenarios: Vec<ScenarioType>,
    /// The start base fee
    pub(crate) base_fee: u128,
    /// generator for transactions
    pub(crate) tx_generator: MockTransactionDistribution,
}

impl MockSimulatorConfig {
    /// Generates a set of random addresses
    pub(crate) fn addresses(&self, rng: &mut impl rand::Rng) -> Vec<Address> {
        std::iter::repeat_with(|| Address::random_with(rng)).take(self.num_senders).collect()
    }
}

/// Represents the different types of test scenarios.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub(crate) enum ScenarioType {
    OnchainNonce,
    HigherNonce { skip: u64 },
    BelowBaseFee { fee: u128 },
    FillNonceGap,
}

/// The actual scenario, ready to be executed
///
/// A scenario produces one or more transactions and expects a certain Outcome.
///
/// An executed scenario can affect previous executed transactions
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub(crate) enum Scenario {
    /// Send a tx with the same nonce as on chain.
    OnchainNonce { nonce: u64 },
    /// Send a tx with a higher nonce that what the sender has on chain
    HigherNonce { onchain: u64, nonce: u64 },
    /// Send a tx with a base fee below the base fee of the pool
    BelowBaseFee { fee: u128 },
    /// Fill a nonce gap to promote queued transactions
    FillNonceGap { filled_nonce: u64, promoted_nonce: u64 },
    /// Execute multiple test scenarios
    Multi { scenario: Vec<Self> },
}

/// Represents an executed scenario
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub(crate) struct ExecutedScenario {
    /// balance at the time of execution
    balance: U256,
    /// nonce at the time of execution
    nonce: u64,
    /// The executed scenario
    scenario: Scenario,
}

/// All executed scenarios by a sender
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub(crate) struct ExecutedScenarios {
    sender: Address,
    scenarios: Vec<ExecutedScenario>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{MockFeeRange, MockTransactionRatio};

    #[test]
    fn test_on_chain_nonce_scenario() {
        let transaction_ratio = MockTransactionRatio {
            legacy_pct: 30,
            dynamic_fee_pct: 70,
            access_list_pct: 0,
            blob_pct: 0,
        };

        let base_fee = 10u128;
        let fee_ranges = MockFeeRange {
            gas_price: (base_fee..100).try_into().unwrap(),
            priority_fee: (1u128..10).try_into().unwrap(),
            max_fee: (base_fee..110).try_into().unwrap(),
            max_fee_blob: (1u128..100).try_into().unwrap(),
        };

        let config = MockSimulatorConfig {
            num_senders: 10,
            scenarios: vec![ScenarioType::OnchainNonce],
            base_fee,
            tx_generator: MockTransactionDistribution::new(
                transaction_ratio,
                fee_ranges,
                10..100,
                10..100,
            ),
        };
        let mut simulator = MockTransactionSimulator::new(rand::rng(), config);
        let mut pool = simulator.create_pool();

        simulator.next(&mut pool);
        assert_eq!(pool.pending().len(), 1);
        assert_eq!(pool.queued().len(), 0);
        assert_eq!(pool.base_fee().len(), 0);
    }

    #[test]
    fn test_higher_nonce_scenario() {
        let transaction_ratio = MockTransactionRatio {
            legacy_pct: 30,
            dynamic_fee_pct: 70,
            access_list_pct: 0,
            blob_pct: 0,
        };

        let base_fee = 10u128;
        let fee_ranges = MockFeeRange {
            gas_price: (base_fee..100).try_into().unwrap(),
            priority_fee: (1u128..10).try_into().unwrap(),
            max_fee: (base_fee..110).try_into().unwrap(),
            max_fee_blob: (1u128..100).try_into().unwrap(),
        };

        let config = MockSimulatorConfig {
            num_senders: 10,
            scenarios: vec![ScenarioType::HigherNonce { skip: 1 }],
            base_fee,
            tx_generator: MockTransactionDistribution::new(
                transaction_ratio,
                fee_ranges,
                10..100,
                10..100,
            ),
        };
        let mut simulator = MockTransactionSimulator::new(rand::rng(), config);
        let mut pool = simulator.create_pool();

        simulator.next(&mut pool);
        assert_eq!(pool.pending().len(), 0);
        assert_eq!(pool.queued().len(), 1);
        assert_eq!(pool.base_fee().len(), 0);
    }

    #[test]
    fn test_below_base_fee_scenario() {
        let transaction_ratio = MockTransactionRatio {
            legacy_pct: 30,
            dynamic_fee_pct: 70,
            access_list_pct: 0,
            blob_pct: 0,
        };

        let base_fee = 10u128;
        let fee_ranges = MockFeeRange {
            gas_price: (base_fee..100).try_into().unwrap(),
            priority_fee: (1u128..10).try_into().unwrap(),
            max_fee: (base_fee..110).try_into().unwrap(),
            max_fee_blob: (1u128..100).try_into().unwrap(),
        };

        let config = MockSimulatorConfig {
            num_senders: 10,
            scenarios: vec![ScenarioType::BelowBaseFee { fee: 8 }], /* fee should be in
                                                                     * [MIN_PROTOCOL_BASE_FEE,
                                                                     * base_fee) */
            base_fee,
            tx_generator: MockTransactionDistribution::new(
                transaction_ratio,
                fee_ranges,
                10..100,
                10..100,
            ),
        };
        let mut simulator = MockTransactionSimulator::new(rand::rng(), config);
        let mut pool = simulator.create_pool();

        simulator.next(&mut pool);
        assert_eq!(pool.pending().len(), 0);
        assert_eq!(pool.queued().len(), 0);
        assert_eq!(pool.base_fee().len(), 1);
    }

    #[test]
    fn test_fill_nonce_gap_scenario() {
        let transaction_ratio = MockTransactionRatio {
            legacy_pct: 30,
            dynamic_fee_pct: 70,
            access_list_pct: 0,
            blob_pct: 0,
        };

        let base_fee = 10u128;
        let fee_ranges = MockFeeRange {
            gas_price: (base_fee..100).try_into().unwrap(),
            priority_fee: (1u128..10).try_into().unwrap(),
            max_fee: (base_fee..110).try_into().unwrap(),
            max_fee_blob: (1u128..100).try_into().unwrap(),
        };

        let config = MockSimulatorConfig {
            num_senders: 5,
            scenarios: vec![ScenarioType::HigherNonce { skip: 5 }],
            base_fee,
            tx_generator: MockTransactionDistribution::new(
                transaction_ratio,
                fee_ranges,
                10..100,
                10..100,
            ),
        };
        let mut simulator = MockTransactionSimulator::new(rand::rng(), config);
        let mut pool = simulator.create_pool();

        // create some nonce gaps
        for _ in 0..10 {
            simulator.next(&mut pool);
        }

        let num_gaps = simulator.nonce_gaps.len();

        assert_eq!(pool.pending().len(), 0);
        assert_eq!(pool.queued().len(), num_gaps);
        assert_eq!(pool.base_fee().len(), 0);

        simulator.scenarios = vec![ScenarioType::FillNonceGap];
        for _ in 0..num_gaps {
            simulator.next(&mut pool);
        }

        let expected_pending = num_gaps * 6;
        assert_eq!(pool.pending().len(), expected_pending);
        assert_eq!(pool.queued().len(), 0);
        assert_eq!(pool.base_fee().len(), 0);
    }

    #[test]
    fn test_random_scenarios() {
        let transaction_ratio = MockTransactionRatio {
            legacy_pct: 30,
            dynamic_fee_pct: 70,
            access_list_pct: 0,
            blob_pct: 0,
        };

        let base_fee = 10u128;
        let fee_ranges = MockFeeRange {
            gas_price: (base_fee..100).try_into().unwrap(),
            priority_fee: (1u128..10).try_into().unwrap(),
            max_fee: (base_fee..110).try_into().unwrap(),
            max_fee_blob: (1u128..100).try_into().unwrap(),
        };

        let config = MockSimulatorConfig {
            num_senders: 10,
            scenarios: vec![
                ScenarioType::OnchainNonce,
                ScenarioType::HigherNonce { skip: 2 },
                ScenarioType::BelowBaseFee { fee: 8 },
                ScenarioType::FillNonceGap,
            ],
            base_fee,
            tx_generator: MockTransactionDistribution::new(
                transaction_ratio,
                fee_ranges,
                10..100,
                10..100,
            ),
        };
        let mut simulator = MockTransactionSimulator::new(rand::rng(), config);
        let mut pool = simulator.create_pool();

        for _ in 0..1000 {
            simulator.next(&mut pool);
        }
    }
}
