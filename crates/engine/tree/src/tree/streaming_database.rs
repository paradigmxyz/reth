use reth_errors::ProviderResult;
use reth_primitives::{Account, Address, BlockNumber, Bytecode, StorageKey, StorageValue, B256};
use reth_revm::database::EvmStateProvider;
use tokio::sync::mpsc;

#[derive(Clone, Copy, Debug)]
pub(crate) enum StateAccess {
    Account(Address),
    StorageSlot(Address, B256),
}

pub(crate) struct StreamingDatabase<DB> {
    inner: DB,
    sender: mpsc::UnboundedSender<StateAccess>,
}

impl<DB> StreamingDatabase<DB> {
    pub(crate) fn new(inner: DB, sender: mpsc::UnboundedSender<StateAccess>) -> Self {
        Self { inner, sender }
    }

    pub(crate) fn new_with_rx(inner: DB) -> (Self, mpsc::UnboundedReceiver<StateAccess>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self::new(inner, tx), rx)
    }
}

impl<DB> EvmStateProvider for StreamingDatabase<DB>
where
    DB: EvmStateProvider,
{
    fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>> {
        let _ = self.sender.send(StateAccess::Account(address));
        self.inner.basic_account(address)
    }

    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        let _ = self.sender.send(StateAccess::StorageSlot(account, storage_key));
        self.inner.storage(account, storage_key)
    }

    fn block_hash(&self, number: BlockNumber) -> ProviderResult<Option<B256>> {
        self.inner.block_hash(number)
    }

    fn bytecode_by_hash(&self, code_hash: B256) -> ProviderResult<Option<Bytecode>> {
        self.bytecode_by_hash(code_hash)
    }
}
