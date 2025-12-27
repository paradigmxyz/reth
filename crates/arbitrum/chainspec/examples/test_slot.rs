use alloy_primitives::{keccak256, B256, U256};

fn main() {
    // Test keccak256 of 31 zero bytes
    let input: [u8; 31] = [0u8; 31];
    let hash = keccak256(&input);
    println!("keccak256(31 zeros) = {:?}", hash);
    
    // Check if it matches Go's claimed value
    let go_hash_31 = "0x208229f333a82dd307373f89a1841336e3cdd34027415410943a504c5597a7e8";
    println!("Go claimed hash:     {}", go_hash_31);
    
    // Our hash
    let rust_hash = format!("{:?}", hash);
    println!("Match: {}", rust_hash.to_lowercase() == go_hash_31.to_lowercase());
}
