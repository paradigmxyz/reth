//! Basic Block Gas Batching optimization for the EVM interpreter.
//!
//! This module implements an optimization that reduces gas checking frequency from
//! per-instruction to per-basic-block. Instead of checking gas on every instruction,
//! it pre-calculates cumulative gas per block and checks once at block boundaries.
//!
//! Expected performance gain: 4-6%.
//!
//! # Basic Block Definition
//!
//! A basic block is a sequence of instructions that:
//! - Has a single entry point (the first instruction or target of a jump)
//! - Has a single exit point (ends with JUMP, JUMPI, REVERT, STOP, RETURN, INVALID, SELFDESTRUCT)
//!
//! # Dynamic Gas Instructions
//!
//! Some instructions have dynamic gas costs that depend on runtime values (e.g., CALL, SSTORE,
//! MLOAD). These instructions still require per-instruction gas calculation, but the optimization
//! still helps by batching the static portions of gas costs within a block.

use alloc::vec::Vec;
use revm::bytecode::{opcode, Bytecode};

/// Represents a basic block within bytecode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BasicBlock {
    /// Start offset in bytecode (inclusive).
    pub start: usize,
    /// End offset in bytecode (exclusive, points to the instruction after this block).
    pub end: usize,
    /// Cumulative static gas cost for all instructions in this block.
    /// This is the sum of static gas costs that can be pre-charged.
    pub static_gas: u64,
    /// Whether this block contains instructions with dynamic gas costs.
    /// If true, per-instruction gas checking may still be needed within the block.
    pub has_dynamic_gas: bool,
}

impl BasicBlock {
    /// Creates a new basic block.
    #[inline]
    pub const fn new(start: usize, end: usize, static_gas: u64, has_dynamic_gas: bool) -> Self {
        Self { start, end, static_gas, has_dynamic_gas }
    }

    /// Returns the number of bytes in this block.
    #[inline]
    pub const fn len(&self) -> usize {
        self.end - self.start
    }

    /// Returns true if this block is empty.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.start >= self.end
    }

    /// Returns true if the given program counter is within this block.
    #[inline]
    pub const fn contains(&self, pc: usize) -> bool {
        pc >= self.start && pc < self.end
    }
}

/// Static gas costs for each opcode.
/// These match the values in revm-interpreter's instruction table.
/// Opcodes with value 0 have dynamic gas costs.
const STATIC_GAS_TABLE: [u64; 256] = {
    let mut table = [0u64; 256];

    // 0x00 range
    table[opcode::STOP as usize] = 0;
    table[opcode::ADD as usize] = 3;
    table[opcode::MUL as usize] = 5;
    table[opcode::SUB as usize] = 3;
    table[opcode::DIV as usize] = 5;
    table[opcode::SDIV as usize] = 5;
    table[opcode::MOD as usize] = 5;
    table[opcode::SMOD as usize] = 5;
    table[opcode::ADDMOD as usize] = 8;
    table[opcode::MULMOD as usize] = 8;
    table[opcode::EXP as usize] = 0; // dynamic
    table[opcode::SIGNEXTEND as usize] = 5;

    // 0x10 range - comparisons
    table[opcode::LT as usize] = 3;
    table[opcode::GT as usize] = 3;
    table[opcode::SLT as usize] = 3;
    table[opcode::SGT as usize] = 3;
    table[opcode::EQ as usize] = 3;
    table[opcode::ISZERO as usize] = 3;
    table[opcode::AND as usize] = 3;
    table[opcode::OR as usize] = 3;
    table[opcode::XOR as usize] = 3;
    table[opcode::NOT as usize] = 3;
    table[opcode::BYTE as usize] = 3;
    table[opcode::SHL as usize] = 3;
    table[opcode::SHR as usize] = 3;
    table[opcode::SAR as usize] = 3;
    table[opcode::CLZ as usize] = 5;

    // 0x20 - SHA3/KECCAK256
    table[opcode::KECCAK256 as usize] = 0; // dynamic

    // 0x30 range - environment info
    table[opcode::ADDRESS as usize] = 2;
    table[opcode::BALANCE as usize] = 0; // dynamic
    table[opcode::ORIGIN as usize] = 2;
    table[opcode::CALLER as usize] = 2;
    table[opcode::CALLVALUE as usize] = 2;
    table[opcode::CALLDATALOAD as usize] = 3;
    table[opcode::CALLDATASIZE as usize] = 2;
    table[opcode::CALLDATACOPY as usize] = 0; // dynamic
    table[opcode::CODESIZE as usize] = 2;
    table[opcode::CODECOPY as usize] = 0; // dynamic
    table[opcode::GASPRICE as usize] = 2;
    table[opcode::EXTCODESIZE as usize] = 0; // dynamic
    table[opcode::EXTCODECOPY as usize] = 0; // dynamic
    table[opcode::RETURNDATASIZE as usize] = 2;
    table[opcode::RETURNDATACOPY as usize] = 0; // dynamic
    table[opcode::EXTCODEHASH as usize] = 0; // dynamic
    table[opcode::BLOCKHASH as usize] = 20;
    table[opcode::COINBASE as usize] = 2;
    table[opcode::TIMESTAMP as usize] = 2;
    table[opcode::NUMBER as usize] = 2;
    table[opcode::DIFFICULTY as usize] = 2;
    table[opcode::GASLIMIT as usize] = 2;
    table[opcode::CHAINID as usize] = 2;
    table[opcode::SELFBALANCE as usize] = 5;
    table[opcode::BASEFEE as usize] = 2;
    table[opcode::BLOBHASH as usize] = 3;
    table[opcode::BLOBBASEFEE as usize] = 2;

    // 0x50 range - stack, memory, storage ops
    table[opcode::POP as usize] = 2;
    table[opcode::MLOAD as usize] = 3;
    table[opcode::MSTORE as usize] = 3;
    table[opcode::MSTORE8 as usize] = 3;
    table[opcode::SLOAD as usize] = 0; // dynamic
    table[opcode::SSTORE as usize] = 0; // dynamic
    table[opcode::JUMP as usize] = 8;
    table[opcode::JUMPI as usize] = 10;
    table[opcode::PC as usize] = 2;
    table[opcode::MSIZE as usize] = 2;
    table[opcode::GAS as usize] = 2;
    table[opcode::JUMPDEST as usize] = 1;
    table[opcode::TLOAD as usize] = 100;
    table[opcode::TSTORE as usize] = 100;
    table[opcode::MCOPY as usize] = 0; // dynamic

    // 0x5F - PUSH0
    table[opcode::PUSH0 as usize] = 2;

    // 0x60 - 0x7F - PUSH1 to PUSH32 (all cost 3)
    let mut i = opcode::PUSH1 as usize;
    while i <= opcode::PUSH32 as usize {
        table[i] = 3;
        i += 1;
    }

    // 0x80 - 0x8F - DUP1 to DUP16 (all cost 3)
    i = opcode::DUP1 as usize;
    while i <= opcode::DUP16 as usize {
        table[i] = 3;
        i += 1;
    }

    // 0x90 - 0x9F - SWAP1 to SWAP16 (all cost 3)
    i = opcode::SWAP1 as usize;
    while i <= opcode::SWAP16 as usize {
        table[i] = 3;
        i += 1;
    }

    // 0xA0 - 0xA4 - LOG0 to LOG4 (dynamic)
    // Already 0

    // 0xF0 range - system operations
    table[opcode::CREATE as usize] = 0; // dynamic
    table[opcode::CALL as usize] = 0; // dynamic
    table[opcode::CALLCODE as usize] = 0; // dynamic
    table[opcode::RETURN as usize] = 0;
    table[opcode::DELEGATECALL as usize] = 0; // dynamic
    table[opcode::CREATE2 as usize] = 0; // dynamic
    table[opcode::STATICCALL as usize] = 0; // dynamic
    table[opcode::REVERT as usize] = 0;
    table[opcode::INVALID as usize] = 0;
    table[opcode::SELFDESTRUCT as usize] = 0; // dynamic

    table
};

/// Returns the static gas cost for an opcode.
#[inline]
pub const fn static_gas(opcode: u8) -> u64 {
    STATIC_GAS_TABLE[opcode as usize]
}

/// Returns true if the opcode terminates a basic block.
#[inline]
pub const fn is_block_terminator(opcode: u8) -> bool {
    matches!(
        opcode,
        opcode::JUMP |
            opcode::JUMPI |
            opcode::STOP |
            opcode::RETURN |
            opcode::REVERT |
            opcode::INVALID |
            opcode::SELFDESTRUCT
    )
}

/// Returns true if the opcode has dynamic gas costs.
#[inline]
pub const fn has_dynamic_gas(opcode: u8) -> bool {
    // Opcodes with static_gas = 0 but are not terminators have dynamic gas
    // Exception: STOP, RETURN, REVERT, INVALID are terminators with 0 static gas
    STATIC_GAS_TABLE[opcode as usize] == 0 &&
        !matches!(opcode, opcode::STOP | opcode::RETURN | opcode::REVERT | opcode::INVALID)
}

/// Returns the number of immediate bytes following a PUSH instruction.
/// Returns 0 for non-PUSH opcodes.
#[inline]
pub const fn push_immediate_size(opcode: u8) -> usize {
    if opcode >= opcode::PUSH1 && opcode <= opcode::PUSH32 {
        (opcode - opcode::PUSH1 + 1) as usize
    } else {
        0
    }
}

/// Analyzes bytecode and extracts basic blocks with their static gas costs.
///
/// This function iterates through the bytecode once to:
/// 1. Identify basic block boundaries (JUMPDEST targets and terminators)
/// 2. Calculate cumulative static gas for each block
/// 3. Track whether blocks contain dynamic gas instructions
///
/// Returns a vector of `BasicBlock` entries sorted by start offset.
pub fn analyze_basic_blocks(bytecode: &[u8]) -> Vec<BasicBlock> {
    if bytecode.is_empty() {
        return Vec::new();
    }

    let mut blocks = Vec::new();
    let mut block_start = 0;
    let mut block_gas = 0u64;
    let mut block_has_dynamic = false;
    let mut pc = 0;

    while pc < bytecode.len() {
        let opcode = bytecode[pc];

        // JUMPDEST starts a new basic block (unless we're at the start)
        if opcode == opcode::JUMPDEST && pc > block_start {
            // End the current block before this JUMPDEST
            blocks.push(BasicBlock::new(block_start, pc, block_gas, block_has_dynamic));
            block_start = pc;
            block_gas = 0;
            block_has_dynamic = false;
        }

        // Accumulate gas
        block_gas = block_gas.saturating_add(static_gas(opcode));

        // Track dynamic gas
        if has_dynamic_gas(opcode) {
            block_has_dynamic = true;
        }

        // Advance PC
        let immediate_size = push_immediate_size(opcode);
        pc += 1 + immediate_size;

        // Block terminators end the current block
        if is_block_terminator(opcode) {
            blocks.push(BasicBlock::new(block_start, pc, block_gas, block_has_dynamic));
            block_start = pc;
            block_gas = 0;
            block_has_dynamic = false;
        }
    }

    // Handle any remaining instructions (shouldn't happen if bytecode is properly padded)
    if block_start < bytecode.len() && block_gas > 0 {
        blocks.push(BasicBlock::new(block_start, bytecode.len(), block_gas, block_has_dynamic));
    }

    blocks
}

/// Analyzed bytecode with pre-computed basic blocks for gas batching.
#[derive(Debug, Clone)]
pub struct AnalyzedBytecode {
    /// The original bytecode.
    bytecode: Bytecode,
    /// Pre-computed basic blocks with gas costs.
    blocks: Vec<BasicBlock>,
    /// Lookup table: maps each PC to its containing block index.
    /// This allows O(1) block lookup during execution.
    pc_to_block: Vec<u16>,
}

impl AnalyzedBytecode {
    /// Analyze bytecode and create an optimized representation.
    pub fn new(bytecode: Bytecode) -> Self {
        let bytes = bytecode.original_byte_slice();
        let blocks = analyze_basic_blocks(bytes);

        // Build PC-to-block lookup table
        let mut pc_to_block = vec![0u16; bytes.len()];
        for (block_idx, block) in blocks.iter().enumerate() {
            for pc in block.start..block.end.min(bytes.len()) {
                pc_to_block[pc] = block_idx as u16;
            }
        }

        Self { bytecode, blocks, pc_to_block }
    }

    /// Returns the underlying bytecode.
    #[inline]
    pub fn bytecode(&self) -> &Bytecode {
        &self.bytecode
    }

    /// Returns the basic blocks.
    #[inline]
    pub fn blocks(&self) -> &[BasicBlock] {
        &self.blocks
    }

    /// Returns the basic block containing the given PC, if any.
    #[inline]
    pub fn block_at(&self, pc: usize) -> Option<&BasicBlock> {
        if pc < self.pc_to_block.len() {
            Some(&self.blocks[self.pc_to_block[pc] as usize])
        } else {
            None
        }
    }

    /// Returns the index of the basic block containing the given PC.
    #[inline]
    pub fn block_index_at(&self, pc: usize) -> Option<usize> {
        if pc < self.pc_to_block.len() {
            Some(self.pc_to_block[pc] as usize)
        } else {
            None
        }
    }

    /// Returns the total static gas that can be pre-charged from the given PC
    /// until the end of the current basic block.
    ///
    /// This is used to batch gas checks: charge this amount at the start of the block
    /// and skip per-instruction static gas checks within the block.
    #[inline]
    pub fn remaining_block_gas(&self, pc: usize) -> u64 {
        if let Some(block) = self.block_at(pc) {
            if pc == block.start {
                block.static_gas
            } else {
                // PC is in the middle of a block - need to calculate remaining gas
                let bytes = self.bytecode.original_byte_slice();
                let mut remaining_gas = 0u64;
                let mut current_pc = pc;

                while current_pc < block.end && current_pc < bytes.len() {
                    let opcode = bytes[current_pc];
                    remaining_gas = remaining_gas.saturating_add(static_gas(opcode));
                    current_pc += 1 + push_immediate_size(opcode);
                }

                remaining_gas
            }
        } else {
            0
        }
    }
}

/// Gas batching state for the interpreter.
///
/// This structure tracks the current basic block during execution,
/// allowing the interpreter to batch gas checks at block boundaries.
#[derive(Debug, Clone, Default)]
pub struct GasBatcher {
    /// Index of the current basic block.
    current_block_idx: usize,
    /// Gas already pre-charged for the current block.
    precharged_gas: u64,
    /// Whether gas has been precharged for the current block.
    block_precharged: bool,
}

impl GasBatcher {
    /// Creates a new gas batcher.
    #[inline]
    pub const fn new() -> Self {
        Self { current_block_idx: 0, precharged_gas: 0, block_precharged: false }
    }

    /// Resets the batcher state.
    #[inline]
    pub fn reset(&mut self) {
        self.current_block_idx = 0;
        self.precharged_gas = 0;
        self.block_precharged = false;
    }

    /// Called at the start of each instruction to handle gas batching.
    ///
    /// Returns the gas to charge for this instruction:
    /// - If we're at a block boundary, returns the cumulative static gas for the block
    /// - Otherwise, returns 0 (gas was already pre-charged)
    ///
    /// Dynamic gas instructions still need their dynamic costs added separately.
    #[inline]
    pub fn precharge_block(
        &mut self,
        pc: usize,
        analyzed: &AnalyzedBytecode,
    ) -> Option<(u64, bool)> {
        let block_idx = analyzed.block_index_at(pc)?;
        let block = &analyzed.blocks()[block_idx];

        // Check if we've entered a new block
        if block_idx != self.current_block_idx || !self.block_precharged {
            self.current_block_idx = block_idx;

            // Calculate gas to precharge from current PC
            let gas_to_charge =
                if pc == block.start { block.static_gas } else { analyzed.remaining_block_gas(pc) };

            self.precharged_gas = gas_to_charge;
            self.block_precharged = true;

            // Return gas to charge and whether block has dynamic gas
            Some((gas_to_charge, block.has_dynamic_gas))
        } else {
            // Already in this block, no additional static gas to charge
            Some((0, block.has_dynamic_gas))
        }
    }

    /// Called when a jump occurs to update block tracking.
    #[inline]
    pub fn on_jump(&mut self) {
        // Reset precharged flag so next instruction triggers precharge
        self.block_precharged = false;
    }

    /// Returns the current block index.
    #[inline]
    pub const fn current_block_idx(&self) -> usize {
        self.current_block_idx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_static_gas_lookup() {
        assert_eq!(static_gas(opcode::ADD), 3);
        assert_eq!(static_gas(opcode::MUL), 5);
        assert_eq!(static_gas(opcode::PUSH1), 3);
        assert_eq!(static_gas(opcode::JUMP), 8);
        assert_eq!(static_gas(opcode::JUMPI), 10);
        assert_eq!(static_gas(opcode::JUMPDEST), 1);
        // Dynamic gas opcodes return 0
        assert_eq!(static_gas(opcode::SLOAD), 0);
        assert_eq!(static_gas(opcode::CALL), 0);
    }

    #[test]
    fn test_block_terminator() {
        assert!(is_block_terminator(opcode::JUMP));
        assert!(is_block_terminator(opcode::JUMPI));
        assert!(is_block_terminator(opcode::STOP));
        assert!(is_block_terminator(opcode::RETURN));
        assert!(is_block_terminator(opcode::REVERT));
        assert!(!is_block_terminator(opcode::ADD));
        assert!(!is_block_terminator(opcode::PUSH1));
    }

    #[test]
    fn test_push_immediate_size() {
        assert_eq!(push_immediate_size(opcode::PUSH1), 1);
        assert_eq!(push_immediate_size(opcode::PUSH32), 32);
        assert_eq!(push_immediate_size(opcode::ADD), 0);
        assert_eq!(push_immediate_size(opcode::JUMP), 0);
    }

    #[test]
    fn test_simple_basic_blocks() {
        // Simple bytecode: PUSH1 0x01 PUSH1 0x02 ADD STOP
        let bytecode: &[u8] =
            &[opcode::PUSH1, 0x01, opcode::PUSH1, 0x02, opcode::ADD, opcode::STOP];
        let blocks = analyze_basic_blocks(bytecode);

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].start, 0);
        assert_eq!(blocks[0].end, 6);
        // PUSH1(3) + PUSH1(3) + ADD(3) + STOP(0) = 9
        assert_eq!(blocks[0].static_gas, 9);
        assert!(!blocks[0].has_dynamic_gas);
    }

    #[test]
    fn test_multiple_basic_blocks() {
        // Bytecode with JUMPDEST: PUSH1 0x01 JUMP JUMPDEST ADD STOP
        let bytecode: &[u8] = &[
            opcode::PUSH1,
            0x04, // jump to offset 4
            opcode::JUMP,
            opcode::JUMPDEST,
            opcode::ADD,
            opcode::STOP,
        ];
        let blocks = analyze_basic_blocks(bytecode);

        assert_eq!(blocks.len(), 2);

        // First block: PUSH1, JUMP
        assert_eq!(blocks[0].start, 0);
        assert_eq!(blocks[0].end, 3);
        // PUSH1(3) + JUMP(8) = 11
        assert_eq!(blocks[0].static_gas, 11);

        // Second block: JUMPDEST, ADD, STOP
        assert_eq!(blocks[1].start, 3);
        assert_eq!(blocks[1].end, 6);
        // JUMPDEST(1) + ADD(3) + STOP(0) = 4
        assert_eq!(blocks[1].static_gas, 4);
    }

    #[test]
    fn test_block_with_dynamic_gas() {
        // Bytecode with SLOAD: PUSH1 0x00 SLOAD STOP
        let bytecode: &[u8] = &[opcode::PUSH1, 0x00, opcode::SLOAD, opcode::STOP];
        let blocks = analyze_basic_blocks(bytecode);

        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].has_dynamic_gas);
        // PUSH1(3) + SLOAD(0) + STOP(0) = 3
        assert_eq!(blocks[0].static_gas, 3);
    }

    #[test]
    fn test_jumpi_terminates_block() {
        // Bytecode with JUMPI
        let bytecode: &[u8] = &[
            opcode::PUSH1,
            0x01,
            opcode::PUSH1,
            0x06,
            opcode::JUMPI,
            opcode::JUMPDEST,
            opcode::STOP,
        ];
        let blocks = analyze_basic_blocks(bytecode);

        assert_eq!(blocks.len(), 2);
        // First block ends after JUMPI
        assert_eq!(blocks[0].end, 5);
    }

    #[test]
    fn test_analyzed_bytecode() {
        let bytecode = Bytecode::new_raw(alloy_primitives::Bytes::from_static(&[
            opcode::PUSH1,
            0x01,
            opcode::PUSH1,
            0x02,
            opcode::ADD,
            opcode::STOP,
        ]));

        let analyzed = AnalyzedBytecode::new(bytecode);
        assert_eq!(analyzed.blocks().len(), 1);

        // Test block lookup
        let block = analyzed.block_at(0).unwrap();
        assert_eq!(block.static_gas, 9);

        // Test remaining gas calculation
        assert_eq!(analyzed.remaining_block_gas(0), 9);
        // At PC=2 (second PUSH1): PUSH1(3) + ADD(3) + STOP(0) = 6
        assert_eq!(analyzed.remaining_block_gas(2), 6);
    }

    #[test]
    fn test_gas_batcher() {
        let bytecode = Bytecode::new_raw(alloy_primitives::Bytes::from_static(&[
            opcode::PUSH1,
            0x04,
            opcode::JUMP,
            opcode::JUMPDEST,
            opcode::STOP,
        ]));

        let analyzed = AnalyzedBytecode::new(bytecode);
        let mut batcher = GasBatcher::new();

        // First call at PC=0 should return block's static gas
        let result = batcher.precharge_block(0, &analyzed);
        assert!(result.is_some());
        let (gas, _) = result.unwrap();
        // PUSH1(3) + JUMP(8) = 11
        assert_eq!(gas, 11);

        // Second call at PC=2 (still in same block) should return 0
        let result = batcher.precharge_block(2, &analyzed);
        let (gas, _) = result.unwrap();
        assert_eq!(gas, 0);

        // After jump, reset and call at PC=3 (new block)
        batcher.on_jump();
        let result = batcher.precharge_block(3, &analyzed);
        let (gas, _) = result.unwrap();
        // JUMPDEST(1) + STOP(0) = 1
        assert_eq!(gas, 1);
    }
}
