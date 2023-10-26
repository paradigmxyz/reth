#!/bin/bash

# RPCs
OP_RETH="http://anton.clab.by:9545"
BASE_GOERLI="https://mainnet.base.org"
CHAIN="Base Mainnet"

# Alter to bisect
NOW=$(cast block-number --rpc-url $OP_RETH)
BLOCK_DIFF=$(diff <(cast block $NOW --rpc-url $OP_RETH) <(cast block $NOW --rpc-url $BASE_GOERLI))

# Divergence watcher (debug)
FILE_PATH="./.divergence"

echo "Chain: $CHAIN"

# Check if BLOCK_DIFF is empty
if [[ -z "$BLOCK_DIFF" ]]; then
        echo "No diff in block #$NOW between op-reth and canonical $CHAIN RPC."
else
        if [[ ! -f "$FILE_PATH" ]]; then
                echo "Diverged around block #$NOW \n $BLOCK_DIFF" > "$FILE_PATH"
                echo "Note - Not precise, bisect in the range of [-500, 500] relative to $NOW to find the actual divergence"
                exit 1
        fi

        echo "-----------------------------------------------------------------------"
        echo " -> ERROR! Diff in block #$NOW between op-reth and canonical $CHAIN RPC"
        echo "$BLOCK_DIFF"
        echo "-----------------------------------------------------------------------"
fi
