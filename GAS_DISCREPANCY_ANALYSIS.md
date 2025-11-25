# Block 2 Gas Discrepancy Analysis

## Problem Statement
Block 2 consistently shows a 6 gas difference between local implementation and official Arbitrum Sepolia:
- **Local**: 0x10a29 (68,137 gas)
- **Official**: 0x10a23 (68,131 gas)
- **Difference**: 6 gas

## Transaction Details

### Transaction 0 (Internal - 0x6a)
- Hash: `0x61e5ec1fa120ba1050e3fb4967f4fb26df773cedcc666b48b4835a378d5b36a2`
- Type: Internal (0x6a)
- Gas Used: 0
- Cumulative: 0
- **Status**: ✅ Matches official

### Transaction 1 (Legacy - 0x0)
- Hash: `0xeddf9e61fb9d8f5111840daef55e5fde0041f5702856532cdbb5a02998033d26`
- Type: Legacy (0x0)
- To: null (contract creation)
- Gas Limit: 0x186a0 (100,000)
- Input: `0x604580600e600039806000f350fe7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe03601600081602082378035828234f58015156039578182fd5b8082525050506014600cf3`
- **Local Gas Used**: 0x10a29 (68,137)
- **Official Gas Used**: 0x10a23 (68,131)
- **Discrepancy**: 6 gas

## Possible Causes

### 1. Opcode Gas Cost Differences
The bytecode contains various opcodes. Possible differences:
- PUSH operations
- DUP operations  
- CODECOPY
- Memory expansion
- RETURN

### 2. Code Deployment Gas
- Standard Ethereum: 200 gas per byte of deployed code
- Possible Arbitrum override or modification

### 3. Intrinsic Gas Calculation
- Base transaction cost: 21,000 gas
- Calldata gas: 4 per zero byte, 16 per non-zero byte
- Contract creation gas: 32,000 gas
- Possible Arbitrum-specific modifications

### 4. EIP-2929 Access Lists
- Cold storage access: 2,100 gas
- Warm storage access: 100 gas
- Difference: 2,000 gas (but we see 6 gas)

### 5. Memory Expansion
- Memory gas = (memory_size_word^2) / 512 + (3 * memory_size_word)
- Small rounding differences could cause 6 gas delta

### 6. Revm vs Go-Ethereum Differences
- Different EVM implementations may have slight variations
- Especially in edge cases or complex calculations

## Investigation Steps

1. ✅ Verified transaction hashes match
2. ✅ Verified all other fields match except gas
3. ✅ Confirmed difference is consistent (6 gas, not variable)
4. ⏳ Need to add execution tracing to see gas consumption per opcode
5. ⏳ Need to compare with official Go implementation step-by-step
6. ⏳ Need to check revm version and any known gas calculation issues

## Code Locations to Review

### Gas Calculation
- `/home/dev/reth/crates/arbitrum/evm/src/build.rs` - Lines 458-492 (cumulative gas)
- `/home/dev/reth/crates/arbitrum/evm/src/execute.rs` - Gas charging logic
- `/home/dev/reth/crates/arbitrum/chainspec/src/lib.rs` - Chain configuration

### EVM Execution
- Revm library (external dependency)
- Contract creation opcode handling
- Memory expansion calculations

## Next Steps

1. **Add Detailed Tracing**: Instrument code to log gas consumption per EVM operation
2. **Compare with Go**: Set up parallel execution trace with official Nitro node
3. **Test in Isolation**: Create minimal test case with just this contract creation
4. **Check Revm**: Review revm version and changelog for gas calculation changes
5. **Consult Arbitrum**: Check if 6 gas difference is known/expected behavior

## Impact

- **Block Hash**: ❌ Causes entire block hash to be different
- **State Root**: ❌ Affects state transitions and merkleization
- **Sync**: ✅ Node syncs successfully despite mismatch
- **Functionality**: ✅ Transaction executes correctly
- **Severity**: Medium - Does not break sync but prevents exact matching

## Conclusion

The 6 gas discrepancy is consistent and reproducible but does not prevent the node from functioning. It suggests a systematic difference in gas calculation between the Rust implementation (revm) and the official Go implementation (go-ethereum/geth). Further investigation with detailed execution tracing is required to identify the exact source.
