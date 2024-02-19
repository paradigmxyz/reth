//! Types for genesis configuration of a chain.

// re-export genesis types
#[doc(inline)]
pub use alloy_genesis::*;

#[cfg(any(test, feature = "test-utils"))]
pub use allocator::GenesisAllocator;

#[cfg(any(test, feature = "test-utils"))]
mod allocator {
    use crate::{public_key_to_address, Address, Bytes, B256, U256};
    use alloy_genesis::GenesisAccount;
    use secp256k1::{
        rand::{thread_rng, RngCore},
        KeyPair, Secp256k1,
    };
    use std::collections::{hash_map::Entry, HashMap};

    /// This helps create a custom genesis alloc by making it easy to add funded accounts with known
    /// signers to the genesis block.
    ///
    /// # Example
    /// ```
    /// # use reth_primitives::{ genesis::GenesisAllocator, Address, U256, hex, Bytes};
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
    #[derive(Debug)]
    pub struct GenesisAllocator<'a> {
        /// The genesis alloc to be built.
        alloc: HashMap<Address, GenesisAccount>,
        /// The rng to use for generating key pairs.
        rng: Box<dyn RngDebug + 'a>,
    }

    impl<'a> GenesisAllocator<'a> {
        /// Initialize a new alloc builder with the provided rng.
        pub fn new_with_rng<R>(rng: &'a mut R) -> Self
        where
            R: RngCore + std::fmt::Debug,
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
        pub fn new_funded_account(&mut self, balance: U256) -> (KeyPair, Address) {
            let secp = Secp256k1::new();
            let pair = KeyPair::new(&secp, &mut self.rng);
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
        ) -> (KeyPair, Address) {
            let secp = Secp256k1::new();
            let pair = KeyPair::new(&secp, &mut self.rng);
            let address = public_key_to_address(pair.public_key());

            self.alloc.insert(
                address,
                GenesisAccount::default().with_balance(balance).with_code(Some(code)),
            );

            (pair, address)
        }

        /// Adds a funded account to the genesis alloc with the provided storage.
        ///
        /// Returns the key pair for the account and the account's address.
        pub fn new_funded_account_with_storage(
            &mut self,
            balance: U256,
            storage: HashMap<B256, B256>,
        ) -> (KeyPair, Address) {
            let secp = Secp256k1::new();
            let pair = KeyPair::new(&secp, &mut self.rng);
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
            storage: HashMap<B256, B256>,
        ) -> (KeyPair, Address) {
            let secp = Secp256k1::new();
            let pair = KeyPair::new(&secp, &mut self.rng);
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
        pub fn new_account_with_code(&mut self, code: Bytes) -> (KeyPair, Address) {
            let secp = Secp256k1::new();
            let pair = KeyPair::new(&secp, &mut self.rng);
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
            let pair = KeyPair::new(&secp, &mut self.rng);
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

    /// Helper trait that encapsulates [RngCore], and [Debug](std::fmt::Debug) to get around rules
    /// for auto traits (Opt-in built-in traits).
    trait RngDebug: RngCore + std::fmt::Debug {}

    impl<T> RngDebug for T where T: RngCore + std::fmt::Debug {}
}
