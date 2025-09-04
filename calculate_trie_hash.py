#!/usr/bin/env python3
"""
Calculate Ethereum trie node hash using RLP + Keccak256
"""

from eth_hash.auto import keccak
import rlp

def calculate_trie_node_hash():
    """Calculate hash for the specific trie node data"""
    
    print("Ethereum Trie Node Hash Calculator")
    print("=" * 50)
    
    # Your specific node data
    key = bytes.fromhex("f3")
    value = bytes.fromhex("f843a020f7a9fe364faab93b216da50a3214154f22a0a2b415b23a84c8169e8b636ee3a1a06884c97b00000000000000000000000000010000000000000000000000fd9688")
    
    print("Input Data:")
    print(f"  Key (nibbles): 0x{key.hex()}")
    print(f"  Value: 0x{value.hex()}")
    print(f"  Value length: {len(value)} bytes")
    print()
    
    # Analyze the node type
    analyze_node_type(key)
    print()
    
    # Create node structure for RLP encoding
    # Ethereum trie nodes are encoded as [key, value] for leaf/extension nodes
    node_data = [key, value]
    
    print("Step 1: RLP Encoding")
    print("-" * 20)
    
    # RLP encode the node
    rlp_encoded = rlp.encode(node_data)
    print(f"RLP encoded: 0x{rlp_encoded.hex()}")
    print(f"RLP length: {len(rlp_encoded)} bytes")
    print()
    
    print("Step 2: Keccak256 Hashing")
    print("-" * 25)
    
    # Keccak256 hash of the RLP-encoded data
    node_hash = keccak(rlp_encoded)
    result = f"0x{node_hash.hex()}"
    
    print(f"Final hash: {result}")
    print(f"Hash length: {len(node_hash)} bytes")
    print()
    
    print("Verification:")
    print("-" * 12)
    print(f"Starts with 0xcc: {result.startswith('0xcc')}")
    print(f"First 8 chars: {result[:10]}")
    
    return result

def analyze_node_type(key):
    """Analyze the trie node type based on key encoding"""
    if len(key) == 0:
        print("Node type: Unknown (empty key)")
        return
    
    first_byte = key[0]
    first_nibble = (first_byte >> 4) & 0x0F  # Upper 4 bits
    second_nibble = first_byte & 0x0F        # Lower 4 bits
    
    print("Node Type Analysis:")
    print(f"  First byte: 0x{first_byte:02x}")
    print(f"  First nibble: 0x{first_nibble:x}")
    print(f"  Second nibble: 0x{second_nibble:x}")
    
    # Ethereum trie key encoding (HP encoding):
    # 0x0X = Extension node with even key length
    # 0x1X = Extension node with odd key length
    # 0x2X = Leaf node with even key length
    # 0x3X = Leaf node with odd key length
    
    if first_nibble == 0x0:
        print("  Type: Extension node (even key length)")
    elif first_nibble == 0x1:
        print("  Type: Extension node (odd key length)")
    elif first_nibble == 0x2:
        print("  Type: Leaf node (even key length)")
    elif first_nibble == 0x3:
        print("  Type: Leaf node (odd key length)")
    elif first_nibble == 0xF:
        if second_nibble == 0x3:
            print("  Type: Leaf node (0xf3 prefix)")
        else:
            print(f"  Type: Unknown (0xf{second_nibble:x} prefix)")
    else:
        print(f"  Type: Unknown prefix (0x{first_nibble:x}{second_nibble:x})")

def test_simple_case():
    """Test with a simpler case for verification"""
    print("\n" + "=" * 50)
    print("Testing Simple Case")
    print("=" * 50)
    
    # Simple test case
    simple_key = bytes.fromhex("20")  # Even simpler key
    simple_value = bytes.fromhex("01")  # Simple value
    
    node = [simple_key, simple_value]
    rlp_encoded = rlp.encode(node)
    hash_result = keccak(rlp_encoded)
    
    print(f"Simple key: 0x{simple_key.hex()}")
    print(f"Simple value: 0x{simple_value.hex()}")
    print(f"RLP: 0x{rlp_encoded.hex()}")
    print(f"Hash: 0x{hash_result.hex()}")

if __name__ == "__main__":
    # Calculate the hash for your specific data
    result = calculate_trie_node_hash()
    
    # Test with simpler case for verification
    test_simple_case()
    
    print(f"\nFinal Result: {result}")