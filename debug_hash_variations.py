#!/usr/bin/env python3
"""
Debug different interpretations of the trie node data to find the 0xcc... hash
"""

from eth_hash.auto import keccak
import rlp

def test_variations():
    """Test different ways to interpret the trie node data"""
    
    print("Testing Different Trie Node Interpretations")
    print("=" * 60)
    
    # Original data
    key_hex = "f3"
    value_hex = "f843a020f7a9fe364faab93b216da50a3214154f22a0a2b415b23a84c8169e8b636ee3a1a06884c97b00000000000000000000000000010000000000000000000000fd9688"
    
    print(f"Original key: {key_hex}")
    print(f"Original value: {value_hex}")
    print()
    
    variations = []
    
    # Variation 1: Original interpretation (leaf node)
    key1 = bytes.fromhex(key_hex)
    value1 = bytes.fromhex(value_hex)
    node1 = [key1, value1]
    hash1 = calculate_hash(node1, "Leaf node [key, value]")
    variations.append(("Leaf [key, value]", hash1))
    
    # Variation 2: Maybe it's just the value that gets hashed
    hash2 = calculate_hash(value1, "Just the value")
    variations.append(("Just value", hash2))
    
    # Variation 3: Maybe the key is different format (nibbles)
    # Convert f3 to actual nibbles [f, 3]
    key3 = bytes([0x0f, 0x03])  # f3 as separate nibbles
    node3 = [key3, value1]
    hash3 = calculate_hash(node3, "Nibbles as separate bytes")
    variations.append(("Nibbles separate", hash3))
    
    # Variation 4: Maybe it's a branch node child reference
    # Just hash the raw value
    hash4 = keccak(value1)
    result4 = f"0x{hash4.hex()}"
    print(f"Raw value hash: {result4}")
    variations.append(("Raw value hash", result4))
    
    # Variation 5: Maybe key needs HP encoding prefix removed
    # If 0xf3 is HP encoded, the actual path might be just [3]
    if key_hex.startswith('f'):
        # Remove HP encoding - f3 means leaf with path [3]
        actual_path = key_hex[1:]  # Remove 'f', keep '3'
        key5 = bytes.fromhex('3' if len(actual_path) % 2 == 1 else actual_path)
        node5 = [key5, value1]
        hash5 = calculate_hash(node5, "HP decoded key")
        variations.append(("HP decoded", hash5))
    
    # Variation 6: Maybe the entire thing is RLP encoded differently
    # Try encoding as a branch node (17 elements)
    branch_node = [b''] * 16 + [value1]  # 16 empty children + value
    branch_node[int(key_hex, 16) % 16] = value1  # Put value in appropriate child
    hash6 = calculate_hash(branch_node, "As branch node")
    variations.append(("Branch node", hash6))
    
    # Variation 7: Maybe it's the concatenation
    concatenated = key1 + value1
    hash7 = keccak(concatenated)
    result7 = f"0x{hash7.hex()}"
    print(f"Concatenated key+value: {result7}")
    variations.append(("Key+value concat", result7))
    
    print("\n" + "=" * 60)
    print("SUMMARY - Looking for 0xcc... prefix:")
    print("=" * 60)
    
    cc_found = False
    for desc, hash_val in variations:
        starts_cc = hash_val.startswith('0xcc')
        print(f"{desc:20} : {hash_val[:12]}... (cc: {starts_cc})")
        if starts_cc:
            cc_found = True
            print(f"  *** FOUND 0xcc MATCH: {hash_val}")
    
    if not cc_found:
        print("\nNo variation produced 0xcc... hash.")
        print("The expected hash might:")
        print("1. Use different input data")
        print("2. Use different hashing (not Keccak256)")
        print("3. Be from a different node type entirely")

def calculate_hash(data, description):
    """Calculate hash and print details"""
    try:
        if isinstance(data, (list, tuple)):
            rlp_encoded = rlp.encode(data)
            hash_result = keccak(rlp_encoded)
        else:
            hash_result = keccak(data)
        
        result = f"0x{hash_result.hex()}"
        print(f"{description:25} : {result[:12]}...")
        return result
        
    except Exception as e:
        print(f"{description:25} : ERROR - {e}")
        return "ERROR"

def analyze_expected_cc_hash():
    """Try to reverse engineer what could produce a 0xcc... hash"""
    print("\n" + "=" * 60)  
    print("REVERSE ENGINEERING 0xcc... HASH")
    print("=" * 60)
    
    # Test what kind of data might produce 0xcc prefix
    test_inputs = [
        b"test",
        bytes([0xcc]),
        bytes.fromhex("cc"),
        bytes.fromhex("f3"),
        b"f3",
        rlp.encode([b"f3"]),
        rlp.encode([bytes.fromhex("f3")]),
    ]
    
    for i, test_input in enumerate(test_inputs):
        hash_result = keccak(test_input)
        result = f"0x{hash_result.hex()}"
        print(f"Test {i+1:2} ({test_input!r:12}): {result[:12]}... (cc: {result.startswith('0xcc')})")

if __name__ == "__main__":
    test_variations()
    analyze_expected_cc_hash()