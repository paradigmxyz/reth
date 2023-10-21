//! Tools for the Ephemery testnet.

use crate::{Chain, ChainSpec, ForkCondition, Hardfork};
use eyre::{eyre, Result};
use lazy_static::lazy_static;
use std::{
    sync::{Mutex, Arc},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const PERIOD: u64 = 604800;

/// Structure storing current Ephemery configuration.
struct EphemeryConfig {
    config_update: bool,
    iteration: u64,
    current_id: u64,
}

lazy_static! {
    static ref EPHEMERY_CONFIG: Mutex<EphemeryConfig> = Mutex::new(EphemeryConfig {
        iteration: u64::MAX,
        config_update: false,
        current_id: 39438000,
    });
}

/// Checks the Ephemery [ChainSpec] and updates it if the iteration has changed.
pub fn check_update_spec(mut spec: Arc<ChainSpec>) -> Result<Arc<ChainSpec>, eyre::Error> {
    let iteration = calculate_iteration(spec.genesis_timestamp())?;
    set_iteration(iteration);
    if genesis_outdated(spec.genesis_timestamp()) {
        set_config_update(true); // preparation for the reset function
        let new_spec = Arc::make_mut(&mut spec);
        let new_genesis_time = PERIOD * get_iteration()? + new_spec.genesis_timestamp();
        match new_spec.chain {
            Chain::Named(name) => {
                set_current_id(name as u64 + get_iteration()?);
            },
            Chain::Id(_) => {
                return Err(eyre::eyre!("Ephemery: chain id is not named."))
            },
        }
        new_spec.chain = Chain::Id(get_current_id());
        new_spec.fork_timestamps.shanghai = Some(new_genesis_time);
        if let Some(sh_ts) = new_spec.hardforks.get_mut(&Hardfork::Shanghai) {
            *sh_ts = ForkCondition::Timestamp(new_genesis_time);
        } else {
            return Err(eyre::eyre!("Ephemery: Shanghai not in the hardforks."))
        }
        new_spec.genesis.timestamp = new_genesis_time;
        new_spec.genesis.config.chain_id = get_current_id();
        Ok(Arc::new(new_spec.clone()))
    } else {
        Ok(spec)
    }
}

/// Returns true if the genesis is currently outdated based on the given genesis time. 
pub fn genesis_outdated(min_genesis_time: u64) -> bool {
    if min_genesis_time + PERIOD < timestamp_now() {
        true
    } else {
        false
    }
}

/// Returns the current timestamp in seconds.
fn timestamp_now() -> u64 {    
    SystemTime::now().duration_since(UNIX_EPOCH)
    .unwrap_or_else(|_| Duration::from_secs(0))
    .as_secs()
}

/// Returns true if the current [ChainSpec] has the same id as the [current_id] of Ephemery.
pub fn is_ephemery(chain: Chain) -> bool {
    if chain == Chain::Id(get_current_id()) {
        true
    } else {
        false
    }
}

/// Calculates the current Ephemery iteration based on the given genesis time.
pub fn calculate_iteration(genesis_time: u64) -> Result<u64, eyre::Error> {
    Ok((timestamp_now() - genesis_time) / PERIOD as u64)
}

/// Set the current iteration in Ephemery config.
fn set_iteration(iteration: u64) {
    let mut data = EPHEMERY_CONFIG.lock().unwrap();
    data.iteration = iteration;
}

/// Get the current iteration from Ephemery config.
pub fn get_iteration() -> Result<u64, eyre::Error> {
    let data = EPHEMERY_CONFIG.lock().unwrap();
    match data.iteration {
        u64::MAX => Err(eyre!("Invalid Ephemery iteration.")),
        _ => Ok(data.iteration),
    }
}

/// Set the flag if [ChainSpec] has been changed and a db purge is required.
pub fn set_config_update(config_update: bool) {
    let mut data = EPHEMERY_CONFIG.lock().unwrap();
    data.config_update = config_update;
}

/// Get the flag that indicates if [ChainSpec] has been changed and a db purge is required.
pub fn get_config_update() -> bool {
    let data = EPHEMERY_CONFIG.lock().unwrap();
    data.config_update
}

/// Set the current chain id in Ephemery config.
fn set_current_id(current_id: u64) {
    let mut data = EPHEMERY_CONFIG.lock().unwrap();
    data.current_id = current_id;
}

/// Get the current chain id from Ephemery config.
pub fn get_current_id() -> u64 {
    let data = EPHEMERY_CONFIG.lock().unwrap();
    data.current_id
}
