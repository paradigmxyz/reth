use crate::transaction::TransactionTestContext;
use alloy_eips::BlockId;
use alloy_network::Network;
use alloy_primitives::{Address, Bytes, TxKind, U256};
use alloy_provider::Provider;
use alloy_rpc_types_eth::{TransactionInput, TransactionRequest};
use alloy_signer::Signer;
use alloy_signer_local::{coins_bip39::English, MnemonicBuilder, PrivateKeySigner};
use futures_util::future::BoxFuture;
use std::future::IntoFuture;

/// Mnemonic of the test accounts funded by the [`test_genesis`](crate::test_genesis).
pub const TEST_MNEMONIC: &str = "test test test test test test test test test test test junk";

/// One of the accounts of the genesis allocations.
#[derive(Debug)]
pub struct Wallet {
    /// The signer
    pub inner: PrivateKeySigner,
    /// The nonce
    pub inner_nonce: u64,
    /// The chain id
    pub chain_id: u64,
    amount: usize,
}

impl Wallet {
    /// Creates a new account from one of the secret/pubkeys of the genesis allocations (test.json)
    pub fn new(amount: usize) -> Self {
        Self { inner: test_signer(0), chain_id: 1, amount, inner_nonce: 0 }
    }

    /// Sets chain id
    pub const fn with_chain_id(mut self, chain_id: u64) -> Self {
        self.chain_id = chain_id;
        self
    }

    /// Generates a list of wallets
    pub fn wallet_gen(&self) -> Vec<PrivateKeySigner> {
        (0..self.amount as u32).map(|idx| self.signer(idx)).collect()
    }

    /// Returns the signer of the test account at `index`, with the chain id of the wallet.
    pub fn signer(&self, index: u32) -> PrivateKeySigner {
        test_signer(index).with_chain_id(Some(self.chain_id))
    }

    /// Returns the test account at `index`, with the chain id of the wallet and nonce 0.
    pub fn account(&self, index: u32) -> TestAccount {
        TestAccount::new(self.signer(index), self.chain_id)
    }
}

impl Default for Wallet {
    fn default() -> Self {
        Self::new(1)
    }
}

/// Returns the signer of the test account at `index`, derived from [`TEST_MNEMONIC`] with the
/// default Ethereum derivation path `m/44'/60'/0'/0/{index}`.
pub fn test_signer(index: u32) -> PrivateKeySigner {
    MnemonicBuilder::<English>::default()
        .phrase(TEST_MNEMONIC)
        .index(index)
        .expect("valid derivation index")
        .build()
        .expect("valid test mnemonic")
}

/// A test account that tracks its nonce, e.g. one of the accounts funded by the
/// [`test_genesis`](crate::test_genesis).
///
/// [`Self::sign_tx_bytes`] fills the nonce, chain id, gas limit and fees of transaction requests
/// that do not set them, so tests only need to set the fields they care about. The gas limit and
/// fees default to [`Self::DEFAULT_GAS_LIMIT`] and the `DEFAULT_*_FEE_PER_GAS` constants and can be
/// changed once per account with [`Self::with_gas_limit`] and [`Self::with_fees`].
///
/// [`Self::tx`] and its shortcuts [`Self::call`], [`Self::transfer`] and [`Self::deploy`] build and
/// sign a transaction in a single expression:
///
/// ```ignore
/// let mut account = wallet.account(0).with_fees(max_fee_per_gas, max_priority_fee_per_gas);
/// let contract = account.next_contract_address();
/// let deploy = account.deploy(init_code).await;
/// let call = account.call(contract, calldata).gas_limit(100_000).await;
/// ```
#[derive(Debug, Clone)]
pub struct TestAccount {
    signer: PrivateKeySigner,
    chain_id: u64,
    nonce: u64,
    gas_limit: u64,
    max_fee_per_gas: u128,
    max_priority_fee_per_gas: u128,
}

impl TestAccount {
    /// Initial gas limit of transactions signed by [`Self::sign_tx_bytes`] that do not set one.
    pub const DEFAULT_GAS_LIMIT: u64 = 1_000_000;

    /// Initial max fee per gas of transactions signed by [`Self::sign_tx_bytes`] that set no fees.
    ///
    /// High enough that transactions are accepted regardless of the base fee of test chains.
    pub const DEFAULT_MAX_FEE_PER_GAS: u128 = 1_000_000_000_000;

    /// Initial max priority fee per gas of transactions signed by [`Self::sign_tx_bytes`] that set
    /// no fees.
    pub const DEFAULT_MAX_PRIORITY_FEE_PER_GAS: u128 = 1_000_000_000;

    /// Creates a new account for the given signer and chain id, starting at nonce 0.
    pub const fn new(signer: PrivateKeySigner, chain_id: u64) -> Self {
        Self {
            signer,
            chain_id,
            nonce: 0,
            gas_limit: Self::DEFAULT_GAS_LIMIT,
            max_fee_per_gas: Self::DEFAULT_MAX_FEE_PER_GAS,
            max_priority_fee_per_gas: Self::DEFAULT_MAX_PRIORITY_FEE_PER_GAS,
        }
    }

    /// Sets the gas limit of signed transactions that do not set one.
    pub const fn with_gas_limit(mut self, gas_limit: u64) -> Self {
        self.gas_limit = gas_limit;
        self
    }

    /// Sets the EIP-1559 fees of signed transactions that set neither these fees nor a legacy gas
    /// price.
    pub const fn with_fees(
        mut self,
        max_fee_per_gas: u128,
        max_priority_fee_per_gas: u128,
    ) -> Self {
        self.max_fee_per_gas = max_fee_per_gas;
        self.max_priority_fee_per_gas = max_priority_fee_per_gas;
        self
    }

    /// Returns the signer of the account.
    pub const fn signer(&self) -> &PrivateKeySigner {
        &self.signer
    }

    /// Returns the address of the account.
    pub const fn address(&self) -> Address {
        self.signer.address()
    }

    /// Returns the chain id transactions of the account are signed for.
    pub const fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// Returns the nonce of the next transaction of the account.
    pub const fn nonce(&self) -> u64 {
        self.nonce
    }

    /// Returns the nonce of the next transaction and increments it.
    pub const fn next_nonce(&mut self) -> u64 {
        let nonce = self.nonce;
        self.nonce += 1;
        nonce
    }

    /// Sets the nonce to the pending transaction count of the account on the node behind
    /// `provider`, and returns it.
    pub async fn sync_nonce<N: Network>(
        &mut self,
        provider: &impl Provider<N>,
    ) -> eyre::Result<u64> {
        self.nonce =
            provider.get_transaction_count(self.address()).block_id(BlockId::pending()).await?;
        Ok(self.nonce)
    }

    /// Signs the transaction request, returning the EIP-2718 encoded bytes.
    ///
    /// Fills unset fields: the nonce with [`Self::next_nonce`], the chain id, gas limit and, unless
    /// the request sets a legacy gas price, the EIP-1559 fees of the account. An explicit nonce
    /// does not advance the tracked nonce. Contract creations must set `to`, e.g. with
    /// [`TransactionRequest::create`] or [`Self::deploy`].
    ///
    /// # Panics
    ///
    /// If the request can not be built into a signed transaction.
    pub async fn sign_tx_bytes(&mut self, mut tx: TransactionRequest) -> Bytes {
        if tx.nonce.is_none() {
            tx.nonce = Some(self.next_nonce());
        }
        tx.chain_id.get_or_insert(self.chain_id);
        tx.gas.get_or_insert(self.gas_limit);
        if tx.gas_price.is_none() {
            tx.max_fee_per_gas.get_or_insert(self.max_fee_per_gas);
            tx.max_priority_fee_per_gas.get_or_insert(self.max_priority_fee_per_gas);
        }
        TransactionTestContext::sign_tx_bytes(self.signer.clone(), tx).await
    }

    /// Returns the address of the contract created by the next transaction of the account, if it
    /// is a contract creation that does not set an explicit nonce.
    pub fn next_contract_address(&self) -> Address {
        self.address().create(self.nonce)
    }

    /// Starts building a transaction of the account that sets no fields yet.
    pub fn tx(&mut self) -> TestTx<'_> {
        TestTx { account: self, request: TransactionRequest::default() }
    }

    /// Starts building a call of `to` with the given input.
    pub fn call(&mut self, to: Address, input: impl Into<Bytes>) -> TestTx<'_> {
        self.tx().to(to).input(input)
    }

    /// Starts building a transfer of `value` to `to`.
    pub fn transfer(&mut self, to: Address, value: U256) -> TestTx<'_> {
        self.tx().to(to).value(value)
    }

    /// Starts building a contract creation with the given init code.
    ///
    /// See [`Self::next_contract_address`] for the address of the created contract.
    pub fn deploy(&mut self, init_code: impl Into<Bytes>) -> TestTx<'_> {
        self.tx().input(init_code).map_request(TransactionRequest::create)
    }
}

/// A transaction of a [`TestAccount`] being built, see [`TestAccount::tx`].
///
/// Awaiting it signs the transaction with [`TestAccount::sign_tx_bytes`], like [`Self::sign`],
/// which fills the fields that are not set from the account.
#[derive(Debug)]
#[must_use = "the transaction is only signed when awaited"]
pub struct TestTx<'a> {
    account: &'a mut TestAccount,
    request: TransactionRequest,
}

impl TestTx<'_> {
    /// Sets the recipient.
    pub const fn to(mut self, to: Address) -> Self {
        self.request.to = Some(TxKind::Call(to));
        self
    }

    /// Sets the value transferred with the transaction.
    pub const fn value(mut self, value: U256) -> Self {
        self.request.value = Some(value);
        self
    }

    /// Sets the input, i.e. the calldata of a call or the init code of a contract creation.
    pub fn input(mut self, input: impl Into<Bytes>) -> Self {
        self.request.input = TransactionInput::new(input.into());
        self
    }

    /// Sets the gas limit instead of the gas limit of the account.
    pub const fn gas_limit(mut self, gas_limit: u64) -> Self {
        self.request.gas = Some(gas_limit);
        self
    }

    /// Sets the EIP-1559 fees instead of the fees of the account.
    pub const fn fees(mut self, max_fee_per_gas: u128, max_priority_fee_per_gas: u128) -> Self {
        self.request.max_fee_per_gas = Some(max_fee_per_gas);
        self.request.max_priority_fee_per_gas = Some(max_priority_fee_per_gas);
        self
    }

    /// Sets a legacy gas price, which makes this a legacy transaction without the EIP-1559 fees of
    /// the account.
    pub const fn gas_price(mut self, gas_price: u128) -> Self {
        self.request.gas_price = Some(gas_price);
        self
    }

    /// Sets an explicit nonce, which does not advance the tracked nonce of the account.
    pub const fn nonce(mut self, nonce: u64) -> Self {
        self.request.nonce = Some(nonce);
        self
    }

    /// Modifies the underlying transaction request, e.g. to set fields without a dedicated setter.
    pub fn map_request(mut self, f: impl FnOnce(TransactionRequest) -> TransactionRequest) -> Self {
        self.request = f(self.request);
        self
    }

    /// Signs the transaction, returning the EIP-2718 encoded bytes.
    ///
    /// See [`TestAccount::sign_tx_bytes`].
    pub async fn sign(self) -> Bytes {
        self.account.sign_tx_bytes(self.request).await
    }
}

impl<'a> IntoFuture for TestTx<'a> {
    type Output = Bytes;
    type IntoFuture = BoxFuture<'a, Bytes>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.sign())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{transaction::SignerRecoverable, Transaction, TxEnvelope};
    use alloy_eips::eip2718::Decodable2718;

    fn decode(raw: Bytes) -> TxEnvelope {
        let tx = TxEnvelope::decode_2718(&mut raw.as_ref()).unwrap();
        assert_eq!(tx.recover_signer().unwrap(), Wallet::default().account(0).address());
        tx
    }

    fn assert_send<T: Send>(_: T) {}

    /// Tests of downstream nodes sign transactions in spawned tasks, so the futures must be
    /// `Send`.
    #[expect(dead_code)]
    fn test_sign_futures_are_send(account: &mut TestAccount) {
        assert_send(account.sign_tx_bytes(TransactionRequest::default()));
        assert_send(account.tx().sign());
        assert_send(account.tx().into_future());
    }

    #[tokio::test]
    async fn tx_builder_fills_account_defaults() {
        let mut account = Wallet::default()
            .with_chain_id(1337)
            .account(0)
            .with_gas_limit(300_000)
            .with_fees(7, 3);
        let to = Address::repeat_byte(0x11);

        let call = decode(account.call(to, [0xab, 0xcd]).await);
        assert!(call.is_eip1559());
        assert_eq!(call.chain_id(), Some(1337));
        assert_eq!(call.nonce(), 0);
        assert_eq!(call.kind(), TxKind::Call(to));
        assert_eq!(call.input().as_ref(), [0xab, 0xcd]);
        assert_eq!(call.value(), U256::ZERO);
        assert_eq!(call.gas_limit(), 300_000);
        assert_eq!(call.max_fee_per_gas(), 7);
        assert_eq!(call.max_priority_fee_per_gas(), Some(3));

        let transfer =
            decode(account.transfer(to, U256::from(5)).gas_limit(21_000).fees(9, 1).sign().await);
        assert_eq!(transfer.nonce(), 1);
        assert_eq!(transfer.value(), U256::from(5));
        assert_eq!(transfer.gas_limit(), 21_000);
        assert_eq!(transfer.max_fee_per_gas(), 9);
        assert_eq!(transfer.max_priority_fee_per_gas(), Some(1));

        assert_eq!(account.next_contract_address(), account.address().create(2));
        let deploy = decode(account.deploy([0x00]).await);
        assert_eq!(deploy.nonce(), 2);
        assert_eq!(deploy.kind(), TxKind::Create);
        assert_eq!(deploy.input().as_ref(), [0x00]);

        // An explicit nonce does not advance the tracked nonce.
        let legacy = decode(account.transfer(to, U256::ZERO).gas_price(11).nonce(9).await);
        assert!(legacy.is_legacy());
        assert_eq!(legacy.nonce(), 9);
        assert_eq!(legacy.gas_price(), Some(11));
        assert_eq!(account.nonce(), 3);

        let mapped = decode(account.tx().map_request(|tx| tx.to(to).value(U256::ONE)).await);
        assert_eq!(mapped.nonce(), 3);
        assert_eq!(mapped.value(), U256::ONE);
    }
}
