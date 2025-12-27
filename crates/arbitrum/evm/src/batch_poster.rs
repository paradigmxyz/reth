// BatchPostersTable implementation matching Go nitro's arbos/l1pricing/batchPoster.go
//
// Storage layout:
// - BatchPosterTableKey = [0] (subspace key for the batch poster table)
// - totalFundsDueOffset = 0 (within the batch poster table subspace)
// - PosterAddrsKey = [0] (subspace key for the address set within batch poster table)
// - PosterInfoKey = [1] (subspace key for poster info within batch poster table)
//
// Each poster has:
// - fundsDue at offset 0 within their subspace
// - payTo at offset 1 within their subspace

use alloy_primitives::{Address, B256, U256};
use revm::Database;
use crate::storage::{Storage, StorageBackedBigInt, StorageBackedAddress};
use crate::addressset::AddressSet;

/// Key for the batch poster table subspace within L1 pricing state
pub const BATCH_POSTER_TABLE_KEY: &[u8] = &[0];

/// Key for the poster addresses set within the batch poster table
pub const POSTER_ADDRS_KEY: &[u8] = &[0];

/// Key for the poster info subspace within the batch poster table
pub const POSTER_INFO_KEY: &[u8] = &[1];

/// Offset for totalFundsDue within the batch poster table
const TOTAL_FUNDS_DUE_OFFSET: u64 = 0;

/// Offset for fundsDue within a poster's info subspace
const FUNDS_DUE_OFFSET: u64 = 0;

/// Offset for payTo within a poster's info subspace
const PAY_TO_OFFSET: u64 = 1;

/// BatchPostersTable manages the set of batch posters and their payment state.
/// This matches Go nitro's BatchPostersTable in arbos/l1pricing/batchPoster.go
pub struct BatchPostersTable<D> {
    /// The address set of all batch posters
    poster_addrs: AddressSet<D>,
    /// Storage for poster info (subspace for each poster)
    poster_info: Storage<D>,
    /// Total funds due across all posters (public for L1PricingState access)
    pub total_funds_due: StorageBackedBigInt<D>,
}

impl<D: Database> BatchPostersTable<D> {
    /// Open the batch poster table from the given L1 pricing storage.
    /// The storage should be the L1 pricing subspace.
    pub fn open(l1_pricing_storage: &Storage<D>) -> Self {
        // Open the batch poster table subspace
        let bpt_storage = l1_pricing_storage.open_sub_storage(BATCH_POSTER_TABLE_KEY);
        
        // Open the address set for poster addresses
        let poster_addrs_storage = bpt_storage.open_sub_storage(POSTER_ADDRS_KEY);
        let poster_addrs = AddressSet::open(poster_addrs_storage);
        
        // Open the poster info subspace
        let poster_info = bpt_storage.open_sub_storage(POSTER_INFO_KEY);
        
        // Open the total funds due storage
        let total_funds_due = StorageBackedBigInt::new(
            bpt_storage.state,
            bpt_storage.base_key,
            TOTAL_FUNDS_DUE_OFFSET,
        );
        
        Self {
            poster_addrs,
            poster_info,
            total_funds_due,
        }
    }

    /// Check if an address is a batch poster
    pub fn contains_poster(&self, poster: Address) -> Result<bool, ()> {
        self.poster_addrs.is_member(poster)
    }

    /// Open a poster's state. If createIfNotExist is true and the poster doesn't exist,
    /// it will be added with payTo set to the poster address itself.
    pub fn open_poster(&self, poster: Address, create_if_not_exist: bool) -> Result<BatchPosterState<D>, ()> {
        let is_batch_poster = self.poster_addrs.is_member(poster)?;
        
        if !is_batch_poster {
            if !create_if_not_exist {
                return Err(());
            }
            // Add the poster with payTo = poster
            return self.add_poster(poster, poster);
        }
        
        Ok(self.internal_open(poster))
    }

    /// Add a new batch poster
    pub fn add_poster(&self, poster_address: Address, pay_to: Address) -> Result<BatchPosterState<D>, ()> {
        let is_batch_poster = self.poster_addrs.is_member(poster_address)?;
        if is_batch_poster {
            return Err(()); // Already exists
        }
        
        let bp_state = self.internal_open(poster_address);
        
        // Initialize funds due to 0
        bp_state.funds_due.set(U256::ZERO)?;
        
        // Set pay_to address
        bp_state.pay_to.set(pay_to)?;
        
        // Add to the address set
        self.poster_addrs.add(poster_address)?;
        
        Ok(bp_state)
    }

    /// Internal helper to open a poster's state without checking membership
    fn internal_open(&self, poster: Address) -> BatchPosterState<D> {
        // Open the poster's subspace within poster_info
        // Go nitro uses poster.Bytes() as the subspace key
        let bp_storage = self.poster_info.open_sub_storage(poster.as_slice());
        
        BatchPosterState {
            funds_due: StorageBackedBigInt::new(
                bp_storage.state,
                bp_storage.base_key,
                FUNDS_DUE_OFFSET,
            ),
            pay_to: StorageBackedAddress::new(
                bp_storage.state,
                bp_storage.base_key,
                PAY_TO_OFFSET,
            ),
            posters_table_state: self.total_funds_due.storage,
            posters_table_base_key: B256::from_slice(&{
                // We need the base_key of the batch poster table for total_funds_due updates
                // This is a bit hacky but necessary to match Go nitro's behavior
                let bpt_storage = Storage::<D>::new(self.poster_info.state, self.poster_info.base_key);
                // Go back up one level to get the batch poster table base key
                // Actually, we already have total_funds_due which has the correct slot
                [0u8; 32]
            }),
        }
    }

    /// Get the total funds due across all posters
    pub fn total_funds_due(&self) -> Result<U256, ()> {
        self.total_funds_due.get_raw()
    }

    /// Update total funds due (internal use)
    fn update_total_funds_due(&self, old_poster_due: U256, new_poster_due: U256) -> Result<(), ()> {
        let prev_total = self.total_funds_due.get_raw().unwrap_or(U256::ZERO);
        // new_total = prev_total + new_poster_due - old_poster_due
        let new_total = prev_total.saturating_add(new_poster_due).saturating_sub(old_poster_due);
        self.total_funds_due.set(new_total)
    }
}

/// State for a single batch poster
pub struct BatchPosterState<D> {
    /// Funds due to this poster
    pub funds_due: StorageBackedBigInt<D>,
    /// Address to pay this poster
    pub pay_to: StorageBackedAddress<D>,
    /// Reference to the posters table state for updating total_funds_due
    posters_table_state: *mut revm::database::State<D>,
    posters_table_base_key: B256,
}

impl<D: Database> BatchPosterState<D> {
    /// Get the funds due to this poster
    pub fn get_funds_due(&self) -> Result<U256, ()> {
        self.funds_due.get_raw()
    }

    /// Set the funds due to this poster.
    /// This also updates the total funds due in the posters table.
    pub fn set_funds_due(&self, value: U256, total_funds_due: &StorageBackedBigInt<D>) -> Result<(), ()> {
        let prev = self.funds_due.get_raw().unwrap_or(U256::ZERO);
        let prev_total = total_funds_due.get_raw().unwrap_or(U256::ZERO);
        
        // new_total = prev_total + value - prev
        let new_total = prev_total.saturating_add(value).saturating_sub(prev);
        total_funds_due.set(new_total)?;
        
        self.funds_due.set(value)
    }

    /// Get the pay-to address for this poster
    pub fn get_pay_to(&self) -> Result<Address, ()> {
        self.pay_to.get()
    }

    /// Set the pay-to address for this poster
    pub fn set_pay_to(&self, addr: Address) -> Result<(), ()> {
        self.pay_to.set(addr)
    }
}
