#!/usr/bin/env bash
# Calculate the hash of an Ethereum MPT leaf node
# Usage: ./mpt_leaf_hash.sh <short_key_nibbles> <value_hex>
# Example: ./mpt_leaf_hash.sh "abc" "0xdeadbeef"

set -euo pipefail

CAST="/home/mediocregopher/src/ithaca/foundry/target/debug/cast"

SHORT_KEY="${1:?Usage: $0 <short_key_nibbles> <value_hex>}"
VALUE="${2:?Usage: $0 <short_key_nibbles> <value_hex>}"

# Hex-prefix encode the key for a leaf node
# Leaf flag = 0x20, odd length adds 0x10 and prepends nibble
hp_encode_leaf() {
    local nibbles="$1"
    local len=${#nibbles}
    
    if (( len % 2 == 0 )); then
        # Even length: prefix with 0x20
        echo "0x20${nibbles}"
    else
        # Odd length: prefix with 0x3 merged with first nibble
        echo "0x3${nibbles}"
    fi
}

HP_KEY=$(hp_encode_leaf "$SHORT_KEY")

# RLP encode the value first (storage values are RLP-encoded in the trie)
RLP_VALUE=$("$CAST" to-rlp "$VALUE")

# RLP encode as list [hp_key, rlp_value] then hash
RLP_ENCODED=$("$CAST" to-rlp "[\"$HP_KEY\", \"$RLP_VALUE\"]")
HASH=$("$CAST" keccak "$RLP_ENCODED")

echo "Short key:    $SHORT_KEY"
echo "HP encoded:   $HP_KEY"
echo "Value:        $VALUE"
echo "RLP value:    $RLP_VALUE"
echo "RLP encoded:  $RLP_ENCODED"
echo "Leaf hash:    $HASH"
