use crate::transaction::TransactionTestContext;
use alloy_eips::BlockId;
use alloy_network::Network;
use alloy_primitives::{Address, Bytes};
use alloy_provider::Provider;
use alloy_rpc_types_eth::TransactionRequest;
use alloy_signer::Signer;
use alloy_signer_local::{coins_bip39::English, MnemonicBuilder, PrivateKeySigner};

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
/// that do not set them, so tests only need to set the fields they care about.
#[derive(Debug, Clone)]
pub struct TestAccount {
    signer: PrivateKeySigner,
    chain_id: u64,
    nonce: u64,
}

impl TestAccount {
    /// Gas limit of transactions signed by [`Self::sign_tx_bytes`] that do not set one.
    pub const DEFAULT_GAS_LIMIT: u64 = 1_000_000;

    /// Max fee per gas of transactions signed by [`Self::sign_tx_bytes`] that set no fees.
    ///
    /// High enough that transactions are accepted regardless of the base fee of test chains.
    pub const DEFAULT_MAX_FEE_PER_GAS: u128 = 1_000_000_000_000;

    /// Max priority fee per gas of transactions signed by [`Self::sign_tx_bytes`] that set no
    /// fees.
    pub const DEFAULT_MAX_PRIORITY_FEE_PER_GAS: u128 = 1_000_000_000;

    /// Creates a new account for the given signer and chain id, starting at nonce 0.
    pub const fn new(signer: PrivateKeySigner, chain_id: u64) -> Self {
        Self { signer, chain_id, nonce: 0 }
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
    /// Fills unset fields: the nonce with [`Self::next_nonce`], the chain id of the account, the
    /// gas limit with [`Self::DEFAULT_GAS_LIMIT`] and, unless the request sets a legacy gas price,
    /// the EIP-1559 fees with [`Self::DEFAULT_MAX_FEE_PER_GAS`] and
    /// [`Self::DEFAULT_MAX_PRIORITY_FEE_PER_GAS`]. An explicit nonce does not advance the tracked
    /// nonce. Contract creations must set `to`, e.g. with
    /// [`TransactionBuilder::into_create`](alloy_network::TransactionBuilder::into_create).
    ///
    /// # Panics
    ///
    /// If the request can not be built into a signed transaction.
    pub async fn sign_tx_bytes(&mut self, mut tx: TransactionRequest) -> Bytes {
        if tx.nonce.is_none() {
            tx.nonce = Some(self.next_nonce());
        }
        tx.chain_id.get_or_insert(self.chain_id);
        tx.gas.get_or_insert(Self::DEFAULT_GAS_LIMIT);
        if tx.gas_price.is_none() {
            tx.max_fee_per_gas.get_or_insert(Self::DEFAULT_MAX_FEE_PER_GAS);
            tx.max_priority_fee_per_gas.get_or_insert(Self::DEFAULT_MAX_PRIORITY_FEE_PER_GAS);
        }
        TransactionTestContext::sign_tx_bytes(self.signer.clone(), tx).await
    }
}
