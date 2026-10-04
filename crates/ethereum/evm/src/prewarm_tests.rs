//! SDK runner compatibility using the ordinary Ethereum configuration.

use super::*;
use alloy_primitives::{address, Address, TxKind, B256};
use reth_evm::{
    BoxedPrewarmRunner, Database, Evm, EvmErrorFor, EvmFor, HaltReasonFor, PrewarmRunner,
};
use revm::{
    context::{result::ResultAndState, TxEnv},
    database_interface::DBErrorMarker,
    state::{AccountInfo, Bytecode},
};
use std::{
    cell::Cell,
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
    thread::{self, ThreadId},
};

const IDENTITY: Address = address!("0000000000000000000000000000000000000004");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReadError;

impl core::fmt::Display for ReadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("injected parent read error")
    }
}

impl core::error::Error for ReadError {}
impl DBErrorMarker for ReadError {}

// Rc makes the actual database !Send. The constructor must not acquire a Send bound.
#[derive(Debug, Default)]
struct ParentDb {
    fail: Rc<Cell<bool>>,
    drops: Rc<Cell<usize>>,
}

impl Drop for ParentDb {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

impl revm::Database for ParentDb {
    type Error = ReadError;

    fn basic(&mut self, _: Address) -> Result<Option<AccountInfo>, Self::Error> {
        if self.fail.replace(false) {
            Err(ReadError)
        } else {
            Ok(None)
        }
    }

    fn code_by_hash(&mut self, _: B256) -> Result<Bytecode, Self::Error> {
        Ok(Bytecode::default())
    }

    fn storage(&mut self, _: Address, _: U256) -> Result<U256, Self::Error> {
        Ok(U256::ZERO)
    }

    fn block_hash(&mut self, _: u64) -> Result<B256, Self::Error> {
        Ok(B256::ZERO)
    }
}

fn environment() -> EvmEnv {
    EvmEnv {
        cfg_env: CfgEnv::new().with_spec_and_mainnet_gas_params(SpecId::BERLIN),
        block_env: BlockEnv { gas_limit: 1_000_000, ..Default::default() },
    }
}

fn transaction() -> TxEnv {
    TxEnv {
        caller: Address::repeat_byte(0x11),
        kind: TxKind::Call(IDENTITY),
        gas_limit: 50_000,
        data: Bytes::from_static(b"runner identity input"),
        ..Default::default()
    }
}

#[test]
fn default_runner_preserves_outputs_errors_and_precompile_mutation() {
    let config = EthEvmConfig::mainnet();
    let direct_db = ParentDb::default();
    let direct_fail = direct_db.fail.clone();
    let direct_drops = direct_db.drops.clone();
    let runner_db = ParentDb::default();
    let runner_fail = runner_db.fail.clone();
    let runner_drops = runner_db.drops.clone();
    let mut direct = config.evm_with_env(direct_db, environment());
    let mut runner = config.prewarm_runner(runner_db, environment());
    let tx = transaction();

    // Repeated nonce zero succeeds because neither execution commits its output.
    for step in 0..4 {
        let mut input = tx.clone();
        if step == 1 {
            input.nonce = 1;
        } else if step == 2 {
            direct_fail.set(true);
            runner_fail.set(true);
        }
        let expected = direct.transact(input.clone());
        let actual = runner.transact(input);
        assert_eq!(actual, expected);
        match step {
            1 => assert!(matches!(actual, Err(revm::context::result::EVMError::Transaction(_)))),
            2 => {
                assert!(matches!(actual, Err(revm::context::result::EVMError::Database(ReadError))))
            }
            _ => assert_eq!(actual.unwrap().result.output(), Some(&tx.data)),
        }
    }

    // A caller mutating the returned map must affect the actual executing EVM.
    direct.precompiles_mut().apply_precompile(&IDENTITY, |_| None);
    runner.precompiles_mut().apply_precompile(&IDENTITY, |_| None);
    let expected = direct.transact(tx.clone()).unwrap();
    let actual = runner.transact(tx).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual.result.output(), Some(&Bytes::new()));
    assert_eq!(direct_drops.get(), 0);
    assert_eq!(runner_drops.get(), 0);
    drop(direct);
    drop(runner);
    assert_eq!(direct_drops.get(), 1);
    assert_eq!(runner_drops.get(), 1);
}

#[derive(Clone, Debug)]
struct OverrideConfig {
    inner: EthEvmConfig,
    constructed: Arc<AtomicUsize>,
    dropped: Arc<AtomicUsize>,
    completed_calls: Arc<AtomicUsize>,
    wrong_thread: Arc<AtomicUsize>,
}

impl ConfigureEvm for OverrideConfig {
    type Primitives = EthPrimitives;
    type Error = Infallible;
    type NextBlockEnvCtx = NextBlockEnvAttributes;
    type BlockExecutorFactory = <EthEvmConfig as ConfigureEvm>::BlockExecutorFactory;
    type BlockAssembler = <EthEvmConfig as ConfigureEvm>::BlockAssembler;

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        self.inner.block_executor_factory()
    }

    fn block_assembler(&self) -> &Self::BlockAssembler {
        self.inner.block_assembler()
    }

    fn evm_env(&self, header: &Header) -> Result<EvmEnv, Self::Error> {
        self.inner.evm_env(header)
    }

    fn next_evm_env(
        &self,
        parent: &Header,
        attributes: &Self::NextBlockEnvCtx,
    ) -> Result<EvmEnv, Self::Error> {
        self.inner.next_evm_env(parent, attributes)
    }

    fn context_for_block<'a>(
        &self,
        block: &'a SealedBlock<Block>,
    ) -> Result<EthBlockExecutionCtx<'a>, Self::Error> {
        self.inner.context_for_block(block)
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader,
        attributes: Self::NextBlockEnvCtx,
    ) -> Result<EthBlockExecutionCtx<'_>, Self::Error> {
        self.inner.context_for_next_block(parent, attributes)
    }

    fn prewarm_runner<DB>(&self, db: DB, env: EvmEnv) -> BoxedPrewarmRunner<Self, DB>
    where
        DB: Database + 'static,
        EvmFor<Self, DB>: 'static,
    {
        self.constructed.fetch_add(1, Ordering::Relaxed);
        Box::new(LocalRunner::<DB> {
            inner: self.inner.prewarm_runner(db, env),
            calls: Rc::new(Cell::new(0)),
            owner: thread::current().id(),
            dropped: self.dropped.clone(),
            completed_calls: self.completed_calls.clone(),
            wrong_thread: self.wrong_thread.clone(),
        })
    }
}

// This runner is !Send even when DB is Send; only the configuration is shared.
struct LocalRunner<DB: Database> {
    inner: BoxedPrewarmRunner<EthEvmConfig, DB>,
    calls: Rc<Cell<usize>>,
    owner: ThreadId,
    dropped: Arc<AtomicUsize>,
    completed_calls: Arc<AtomicUsize>,
    wrong_thread: Arc<AtomicUsize>,
}

impl<DB: Database> Drop for LocalRunner<DB> {
    fn drop(&mut self) {
        self.wrong_thread
            .fetch_add(usize::from(self.owner != thread::current().id()), Ordering::Relaxed);
        self.completed_calls.fetch_add(self.calls.get(), Ordering::Relaxed);
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

impl<DB: Database> PrewarmRunner for LocalRunner<DB> {
    type Tx = TxEnv;
    type Error = EvmErrorFor<EthEvmConfig, DB::Error>;
    type HaltReason = HaltReasonFor<EthEvmConfig>;

    fn transact(&mut self, tx: TxEnv) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        assert_eq!(self.owner, thread::current().id());
        self.calls.set(self.calls.get() + 1);
        self.inner.transact(tx)
    }

    fn precompiles_mut(&mut self) -> &mut PrecompilesMap {
        self.inner.precompiles_mut()
    }
}

fn exercise_override<C>(config: &C)
where
    C: ConfigureEvm<BlockExecutorFactory = <EthEvmConfig as ConfigureEvm>::BlockExecutorFactory>,
    EvmFor<C, ParentDb>: 'static,
{
    // The explicit C call prevents autoderef from bypassing the &C/Arc<C> impl under test.
    let mut runner = C::prewarm_runner(config, ParentDb::default(), environment());
    let tx = transaction();
    assert_eq!(runner.transact(tx.clone()).unwrap().result.output(), Some(&tx.data));
    runner.precompiles_mut().apply_precompile(&IDENTITY, |_| None);
    assert_eq!(runner.transact(tx).unwrap().result.output(), Some(&Bytes::new()));
}

#[test]
fn specialized_runner_forwards_through_concrete_reference_and_arc() {
    let config = OverrideConfig {
        inner: EthEvmConfig::mainnet(),
        constructed: Arc::default(),
        dropped: Arc::default(),
        completed_calls: Arc::default(),
        wrong_thread: Arc::default(),
    };
    exercise_override::<OverrideConfig>(&config);
    exercise_override::<&OverrideConfig>(&&config);
    exercise_override::<Arc<OverrideConfig>>(&Arc::new(config.clone()));
    assert_eq!(config.constructed.load(Ordering::Relaxed), 3);
    assert_eq!(config.dropped.load(Ordering::Relaxed), 3);
    assert_eq!(config.completed_calls.load(Ordering::Relaxed), 6);
    assert_eq!(config.wrong_thread.load(Ordering::Relaxed), 0);
}
