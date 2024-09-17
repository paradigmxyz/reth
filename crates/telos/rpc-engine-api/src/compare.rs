use std::collections::HashSet;
use reth_primitives::{Address, B256, U256};
use reth_primitives::revm_primitives::HashMap;
use revm::TransitionAccount;

use crate::structs::{TelosAccountStateTableRow, TelosAccountTableRow};

/// This function compares the state diffs between revm and Telos EVM contract
pub fn compare_state_diffs(
    revm_state_diffs: HashMap<Address,TransitionAccount>,
    statediffs_account: Vec<TelosAccountTableRow>,
    statediffs_accountstate: Vec<TelosAccountStateTableRow>,
    new_addresses_using_create: Vec<(u64,U256)>,
    new_addresses_using_openwallet: Vec<(u64,U256)>,
) -> bool {

    println!("REVM State diffs: {:?}",revm_state_diffs);
    println!("TEVM State diffs account: {:?}",statediffs_account);
    println!("TEVM State diffs accountstate: {:?}",statediffs_accountstate);

    let mut new_addresses_using_openwallet_hashsetset = HashSet::new();
    for row in &new_addresses_using_openwallet {
        new_addresses_using_openwallet_hashsetset.insert(Address::from_word(B256::from(row.1)));
    }

    let mut modified_addresses = HashSet::new();

    for row in &statediffs_account {
        // Skip if address is created using openwallet and is empty
        if new_addresses_using_openwallet_hashsetset.contains(&row.address) && row.balance == U256::ZERO && row.nonce == 0 && row.code.len() == 0 {
            continue;
        }
        modified_addresses.insert(row.address);
    }
    for row in &statediffs_accountstate {
        modified_addresses.insert(row.address);
    }
    
    if modified_addresses.len() != revm_state_diffs.len() {
        panic!("Difference in number of modified addresses");
    }

    for row in statediffs_account {
        // Skip if address is created using openwallet and is empty
        if new_addresses_using_openwallet_hashsetset.contains(&row.address) && row.balance == U256::ZERO && row.nonce == 0 && row.code.len() == 0 {
            continue;
        }
        let revm_side_row = revm_state_diffs.get(&row.address);
        // Key doesn't exist on revm state diffs
        if revm_side_row.is_none() {
            panic!("A modified `account` table row not found on revm state diffs");
        }
        let unrappwed_revm_side_row = revm_side_row.unwrap();
        // Revm state diff is none
        if unrappwed_revm_side_row.info.is_none() {
            panic!("A modified `account` table row found on revm state diffs, but contains no information");
        }
        // Check balance inequality
        if unrappwed_revm_side_row.info.clone().unwrap().balance != row.balance {
            panic!("Difference in balance");
        }
        // Check nonce inequality
        if unrappwed_revm_side_row.info.clone().unwrap().nonce != row.nonce {
            panic!("Difference in nonce");
        }
        // Check code size inequality
        if unrappwed_revm_side_row.info.clone().unwrap().code.is_none() && row.code.len() != 0 || unrappwed_revm_side_row.info.clone().unwrap().code.is_some() && !unrappwed_revm_side_row.info.clone().unwrap().code.unwrap().is_empty() && row.code.len() == 0 {
            panic!("Difference in code existence");
        }
        // // Check code content inequality
        // if unrappwed_revm_side_row.info.clone().unwrap().code.is_some() && !unrappwed_revm_side_row.info.clone().unwrap().code.unwrap().is_empty() && unrappwed_revm_side_row.info.clone().unwrap().code.unwrap().bytes() != row.code {
        //     panic!("Difference in code content, revm: {:?}, tevm: {:?}",unrappwed_revm_side_row.info.clone().unwrap().code.unwrap().bytes(),row.code);
        // }
    }

    for row in statediffs_accountstate {
        let revm_side_row = revm_state_diffs.get(&row.address);
        // Key doesn't exist on revm state diffs
        if revm_side_row.is_none() {
            panic!("A modified `accountstate` table row not found on revm state diffs");
        }
        let unrappwed_revm_side_row = revm_side_row.unwrap();
        // Check key existance
        let storage_row = unrappwed_revm_side_row.storage.get(&row.key);
        if storage_row.is_none() {
            panic!("Key not found on modified storage");
        }
        // Check value inequality
        if storage_row.unwrap().present_value != row.value {
            panic!("Difference in value on modified storage");
        }
    }

    for _row in new_addresses_using_create {

    }
    for _row in new_addresses_using_openwallet {
        
    }

    // Check balance and nonce
    
    return true
}