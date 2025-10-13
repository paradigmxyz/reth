#![allow(unused)]

use alloy_primitives::{Address, U256, B256};
use revm::Database;
use crate::storage::{Storage, StorageBackedUint64, StorageBackedBigUint, StorageBackedAddress};
use crate::l1_pricing::L1PricingState;
use crate::l2_pricing::L2PricingState;
use crate::retryables::RetryableState;
use crate::addressset::AddressSet;
use crate::addresstable::AddressTable;
use crate::merkleaccumulator::MerkleAccumulator;
use crate::blockhash::Blockhashes;
use crate::programs::Programs;
use crate::features::Features;

const VERSION_OFFSET: u64 = 0;
const UPGRADE_VERSION_OFFSET: u64 = 1;
const UPGRADE_TIMESTAMP_OFFSET: u64 = 2;
const NETWORK_FEE_ACCOUNT_OFFSET: u64 = 3;
const CHAIN_ID_OFFSET: u64 = 4;
const GENESIS_BLOCK_NUM_OFFSET: u64 = 5;
const INFRA_FEE_ACCOUNT_OFFSET: u64 = 6;
const BROTLI_COMPRESSION_LEVEL_OFFSET: u64 = 7;
const NATIVE_TOKEN_ENABLED_FROM_TIME_OFFSET: u64 = 8;

const L1_PRICING_SUBSPACE: &[u8] = &[0];
const L2_PRICING_SUBSPACE: &[u8] = &[1];
const RETRYABLES_SUBSPACE: &[u8] = &[2];
const ADDRESS_TABLE_SUBSPACE: &[u8] = &[3];
const CHAIN_OWNER_SUBSPACE: &[u8] = &[4];
const SEND_MERKLE_SUBSPACE: &[u8] = &[5];
const BLOCKHASHES_SUBSPACE: &[u8] = &[6];
const CHAIN_CONFIG_SUBSPACE: &[u8] = &[7];
const PROGRAMS_SUBSPACE: &[u8] = &[8];
const FEATURES_SUBSPACE: &[u8] = &[9];
const NATIVE_TOKEN_OWNER_SUBSPACE: &[u8] = &[10];

pub fn arbos_state_subspace(subspace_id: u8) -> B256 {
    use alloy_primitives::keccak256;
    
    let arbos_addr = Address::new([
        0xA4, 0xB0, 0x5F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFF, 0xFF, 0xFF, 0xFF,
    ]);
    
    let mut preimage = Vec::with_capacity(20 + 1);
    preimage.extend_from_slice(arbos_addr.as_slice());
    preimage.push(subspace_id);
    
    keccak256(&preimage)
}

pub struct ArbosState<D> {
    pub arbos_version: u64,
    pub upgrade_version: StorageBackedUint64<D>,
    pub upgrade_timestamp: StorageBackedUint64<D>,
    pub network_fee_account: StorageBackedAddress<D>,
    pub l1_pricing_state: L1PricingState<D>,
    pub l2_pricing_state: L2PricingState<D>,
    pub retryable_state: RetryableState<D>,
    pub address_table: AddressTable<D>,
    pub chain_owners: AddressSet<D>,
    pub native_token_owners: AddressSet<D>,
    pub send_merkle: MerkleAccumulator<D>,
    pub programs: Programs<D>,
    pub features: Features<D>,
    pub blockhashes: Blockhashes<D>,
    pub chain_id: StorageBackedBigUint<D>,
    pub genesis_block_num: StorageBackedUint64<D>,
    pub infra_fee_account: StorageBackedAddress<D>,
    pub brotli_compression_level: StorageBackedUint64<D>,
    pub native_token_enabled_time: StorageBackedUint64<D>,
    pub backing_storage: Storage<D>,
}

impl<D: Database> ArbosState<D> {
    pub fn open(state: *mut revm::database::State<D>) -> Result<Self, &'static str> {
        let backing_storage = Storage::new(state, B256::ZERO);
        
        let arbos_version = backing_storage.get_by_uint64(VERSION_OFFSET)
            .map(|v| U256::from_be_bytes(v.0).to::<u64>())
            .unwrap_or(0);
        
        if arbos_version == 0 {
            return Err("ArbOS uninitialized");
        }
        
        Ok(Self {
            arbos_version,
            upgrade_version: StorageBackedUint64::new(state, B256::ZERO, UPGRADE_VERSION_OFFSET),
            upgrade_timestamp: StorageBackedUint64::new(state, B256::ZERO, UPGRADE_TIMESTAMP_OFFSET),
            network_fee_account: StorageBackedAddress::new(state, B256::ZERO, NETWORK_FEE_ACCOUNT_OFFSET),
            l1_pricing_state: L1PricingState::open(backing_storage.open_sub_storage(L1_PRICING_SUBSPACE), arbos_version),
            l2_pricing_state: L2PricingState::open(backing_storage.open_sub_storage(L2_PRICING_SUBSPACE)),
            retryable_state: RetryableState::new(state, B256::from_slice(RETRYABLES_SUBSPACE)),
            address_table: AddressTable::open(backing_storage.open_sub_storage(ADDRESS_TABLE_SUBSPACE)),
            chain_owners: AddressSet::open(backing_storage.open_sub_storage(CHAIN_OWNER_SUBSPACE)),
            native_token_owners: AddressSet::open(backing_storage.open_sub_storage(NATIVE_TOKEN_OWNER_SUBSPACE)),
            send_merkle: MerkleAccumulator::open(backing_storage.open_sub_storage(SEND_MERKLE_SUBSPACE)),
            programs: Programs::open(arbos_version, backing_storage.open_sub_storage(PROGRAMS_SUBSPACE)),
            features: Features::open(backing_storage.open_sub_storage(FEATURES_SUBSPACE)),
            blockhashes: Blockhashes::open(backing_storage.open_sub_storage(BLOCKHASHES_SUBSPACE)),
            chain_id: StorageBackedBigUint::new(state, B256::ZERO, CHAIN_ID_OFFSET),
            genesis_block_num: StorageBackedUint64::new(state, B256::ZERO, GENESIS_BLOCK_NUM_OFFSET),
            infra_fee_account: StorageBackedAddress::new(state, B256::ZERO, INFRA_FEE_ACCOUNT_OFFSET),
            brotli_compression_level: StorageBackedUint64::new(state, B256::ZERO, BROTLI_COMPRESSION_LEVEL_OFFSET),
            native_token_enabled_time: StorageBackedUint64::new(state, B256::ZERO, NATIVE_TOKEN_ENABLED_FROM_TIME_OFFSET),
            backing_storage,
        })
    }

    pub fn initialize(
        state: *mut revm::database::State<D>,
        chain_id: U256,
        arbos_version: u64,
    ) -> Result<Self, &'static str> {
        let backing_storage = Storage::new(state, B256::ZERO);
        
        let current_version = backing_storage.get_by_uint64(VERSION_OFFSET)
            .map(|v| U256::from_be_bytes(v.0).to::<u64>())
            .unwrap_or(0);
        
        if current_version != 0 {
            return Err("ArbOS already initialized");
        }
        
        backing_storage.set_by_uint64(VERSION_OFFSET, B256::from(U256::from(arbos_version)))
            .map_err(|_| "Failed to set version")?;
        
        let l1_pricing_sto = backing_storage.open_sub_storage(L1_PRICING_SUBSPACE);
        let initial_l1_base_fee = U256::from(100_000_000_000u64); // 100 Gwei
        let rewards_recipient = Address::ZERO; // Will be set later
        L1PricingState::<D>::initialize(&l1_pricing_sto, rewards_recipient, initial_l1_base_fee);
        
        let l2_pricing_sto = backing_storage.open_sub_storage(L2_PRICING_SUBSPACE);
        let initial_l2_base_fee = U256::from(100_000_000u64); // 0.1 Gwei
        L2PricingState::<D>::initialize(&l2_pricing_sto, initial_l2_base_fee);
        
        AddressSet::<D>::initialize(&backing_storage.open_sub_storage(CHAIN_OWNER_SUBSPACE))
            .map_err(|_| "Failed to initialize chain owners")?;
        
        AddressSet::<D>::initialize(&backing_storage.open_sub_storage(NATIVE_TOKEN_OWNER_SUBSPACE))
            .map_err(|_| "Failed to initialize native token owners")?;
        
        AddressTable::<D>::initialize(&backing_storage.open_sub_storage(ADDRESS_TABLE_SUBSPACE));
        MerkleAccumulator::<D>::initialize(&backing_storage.open_sub_storage(SEND_MERKLE_SUBSPACE));
        Blockhashes::<D>::initialize(&backing_storage.open_sub_storage(BLOCKHASHES_SUBSPACE));
        
        let chain_id_storage = StorageBackedBigUint::new(state, B256::ZERO, CHAIN_ID_OFFSET);
        chain_id_storage.set(chain_id).map_err(|_| "Failed to set chain ID")?;
        
        Self::open(state)
    }

    pub fn get_chain_id(&self) -> Result<U256, ()> {
        self.chain_id.get()
    }

    pub fn get_network_fee_account(&self) -> Result<Address, ()> {
        self.network_fee_account.get()
    }

    pub fn set_network_fee_account(&self, account: Address) -> Result<(), ()> {
        self.network_fee_account.set(account)
    }

    pub fn get_brotli_compression_level(&self) -> Result<u64, ()> {
        self.brotli_compression_level.get()
    }

    pub fn set_brotli_compression_level(&self, level: u64) -> Result<(), ()> {
        self.brotli_compression_level.set(level)
    }

    pub fn get_infra_fee_account(&self) -> Result<Address, ()> {
        self.infra_fee_account.get()
    }

    pub fn set_infra_fee_account(&self, account: Address) -> Result<(), ()> {
        self.infra_fee_account.set(account)
    }
}
