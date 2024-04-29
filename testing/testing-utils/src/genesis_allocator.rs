//! Helps create a custom genesis alloc by making it easy to add funded accounts with known
//! signers to the genesis block.

use alloy_genesis::GenesisAccount;
use reth_primitives::{public_key_to_address, Address, Bytes, B256, U256};
use secp256k1::{
    rand::{thread_rng, RngCore},
    Keypair, Secp256k1,
};
use std::{
    collections::{hash_map::Entry, BTreeMap, HashMap},
    fmt,
};

/// This helps create a custom genesis alloc by making it easy to add funded accounts with known
/// signers to the genesis block.
///
/// # Example
/// ```
/// # use reth_primitives::{Address, U256, hex, Bytes};
/// # use reth_testing_utils::GenesisAllocator;
/// # use std::str::FromStr;
/// let mut allocator = GenesisAllocator::default();
///
/// // This will add a genesis account to the alloc builder, with the provided balance. The
/// // signer for the account will be returned.
/// let (_signer, _addr) = allocator.new_funded_account(U256::from(100_000_000_000_000_000u128));
///
/// // You can also provide code for the account.
/// let code = Bytes::from_str("0x1234").unwrap();
/// let (_second_signer, _second_addr) =
///     allocator.new_funded_account_with_code(U256::from(100_000_000_000_000_000u128), code);
///
/// // You can also add an account with a specific address.
/// // This will not return a signer, since the address is provided by the user and the signer
/// // may be unknown.
/// let addr = "0Ac1dF02185025F65202660F8167210A80dD5086".parse::<Address>().unwrap();
/// allocator.add_funded_account_with_address(addr, U256::from(100_000_000_000_000_000u128));
///
/// // Once you're done adding accounts, you can build the alloc.
/// let alloc = allocator.build();
/// ```
pub struct GenesisAllocator<'a> {
    /// The genesis alloc to be built.
    alloc: HashMap<Address, GenesisAccount>,
    /// The rng to use for generating key pairs.
    rng: Box<dyn RngCore + 'a>,
}

impl<'a> GenesisAllocator<'a> {
    /// Initialize a new alloc builder with the provided rng.
    pub fn new_with_rng<R>(rng: &'a mut R) -> Self
    where
        R: RngCore,
    {
        Self { alloc: HashMap::default(), rng: Box::new(rng) }
    }

    /// Use the provided rng for generating key pairs.
    pub fn with_rng<R>(mut self, rng: &'a mut R) -> Self
    where
        R: RngCore + std::fmt::Debug,
    {
        self.rng = Box::new(rng);
        self
    }

    /// Add a funded account to the genesis alloc.
    ///
    /// Returns the key pair for the account and the account's address.
    pub fn new_funded_account(&mut self, balance: U256) -> (Keypair, Address) {
        let secp = Secp256k1::new();
        let pair = Keypair::new(&secp, &mut self.rng);
        let address = public_key_to_address(pair.public_key());

        self.alloc.insert(address, GenesisAccount::default().with_balance(balance));

        (pair, address)
    }

    /// Add a funded account to the genesis alloc with the provided code.
    ///
    /// Returns the key pair for the account and the account's address.
    pub fn new_funded_account_with_code(
        &mut self,
        balance: U256,
        code: Bytes,
    ) -> (Keypair, Address) {
        let secp = Secp256k1::new();
        let pair = Keypair::new(&secp, &mut self.rng);
        let address = public_key_to_address(pair.public_key());

        self.alloc
            .insert(address, GenesisAccount::default().with_balance(balance).with_code(Some(code)));

        (pair, address)
    }

    /// Adds a funded account to the genesis alloc with the provided storage.
    ///
    /// Returns the key pair for the account and the account's address.
    pub fn new_funded_account_with_storage(
        &mut self,
        balance: U256,
        storage: BTreeMap<B256, B256>,
    ) -> (Keypair, Address) {
        let secp = Secp256k1::new();
        let pair = Keypair::new(&secp, &mut self.rng);
        let address = public_key_to_address(pair.public_key());

        self.alloc.insert(
            address,
            GenesisAccount::default().with_balance(balance).with_storage(Some(storage)),
        );

        (pair, address)
    }

    /// Adds an account with code and storage to the genesis alloc.
    ///
    /// Returns the key pair for the account and the account's address.
    pub fn new_account_with_code_and_storage(
        &mut self,
        code: Bytes,
        storage: BTreeMap<B256, B256>,
    ) -> (Keypair, Address) {
        let secp = Secp256k1::new();
        let pair = Keypair::new(&secp, &mut self.rng);
        let address = public_key_to_address(pair.public_key());

        self.alloc.insert(
            address,
            GenesisAccount::default().with_code(Some(code)).with_storage(Some(storage)),
        );

        (pair, address)
    }

    /// Adds an account with code to the genesis alloc.
    ///
    /// Returns the key pair for the account and the account's address.
    pub fn new_account_with_code(&mut self, code: Bytes) -> (Keypair, Address) {
        let secp = Secp256k1::new();
        let pair = Keypair::new(&secp, &mut self.rng);
        let address = public_key_to_address(pair.public_key());

        self.alloc.insert(address, GenesisAccount::default().with_code(Some(code)));

        (pair, address)
    }

    /// Add a funded account to the genesis alloc with the provided address.
    ///
    /// Neither the key pair nor the account will be returned, since the address is provided by
    /// the user and the signer may be unknown.
    pub fn add_funded_account_with_address(&mut self, address: Address, balance: U256) {
        self.alloc.insert(address, GenesisAccount::default().with_balance(balance));
    }

    /// Adds the given [GenesisAccount] to the genesis alloc.
    ///
    /// Returns the key pair for the account and the account's address.
    pub fn add_account(&mut self, account: GenesisAccount) -> Address {
        let secp = Secp256k1::new();
        let pair = Keypair::new(&secp, &mut self.rng);
        let address = public_key_to_address(pair.public_key());

        self.alloc.insert(address, account);

        address
    }

    /// Gets the account for the provided address.
    ///
    /// If it does not exist, this returns `None`.
    pub fn get_account(&self, address: &Address) -> Option<&GenesisAccount> {
        self.alloc.get(address)
    }

    /// Gets a mutable version of the account for the provided address, if it exists.
    pub fn get_account_mut(&mut self, address: &Address) -> Option<&mut GenesisAccount> {
        self.alloc.get_mut(address)
    }

    /// Gets an [Entry] for the provided address.
    pub fn account_entry(&mut self, address: Address) -> Entry<'_, Address, GenesisAccount> {
        self.alloc.entry(address)
    }

    /// Build the genesis alloc.
    pub fn build(self) -> HashMap<Address, GenesisAccount> {
        self.alloc
    }
}

impl Default for GenesisAllocator<'_> {
    fn default() -> Self {
        Self { alloc: HashMap::default(), rng: Box::new(thread_rng()) }
    }
}

impl fmt::Debug for GenesisAllocator<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GenesisAllocator").field("alloc", &self.alloc).finish_non_exhaustive()
    }
}
