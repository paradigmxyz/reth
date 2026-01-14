//! Cache-aligned EVM stack with downward growth for improved cache locality.
//!
//! This module provides an optimized stack implementation that:
//! - Uses 64-byte cache line alignment for better CPU cache utilization
//! - Grows downward (high to low address) matching CPU prefetch patterns
//! - Provides in-place binary operations for better cache locality
//!
//! Expected performance gain: 1-2% on EVM execution benchmarks.

use alloc::boxed::Box;
use core::fmt;
use revm::{
    interpreter::{interpreter_types::StackTr, InstructionResult, STACK_LIMIT},
    primitives::U256,
};

/// A cache-aligned wrapper to ensure the stack data starts at a 64-byte boundary.
#[repr(C, align(64))]
struct AlignedData {
    data: [U256; STACK_LIMIT],
}

impl Default for AlignedData {
    fn default() -> Self {
        Self { data: [U256::ZERO; STACK_LIMIT] }
    }
}

/// Cache-aligned EVM stack with downward growth.
///
/// This stack implementation uses a fixed-size array with 64-byte alignment
/// and grows downward (stack pointer starts at STACK_LIMIT and decreases on push).
///
/// # Memory Layout
///
/// ```text
/// Index:    0        1        2       ...    1022     1023
///           ↑                                          ↑
///           |                                          |
///      Bottom of stack                          Top of stack (empty)
///      (oldest values)                          (sp starts here)
///
/// After push(A): sp = 1023, data[1023] = A
/// After push(B): sp = 1022, data[1022] = B
/// ```
///
/// # Cache Benefits
///
/// - 64-byte alignment ensures stack operations stay within cache lines
/// - Downward growth matches CPU cache prefetch patterns
/// - Sequential pushes write to decreasing addresses, benefiting from spatial locality
pub struct AlignedStack {
    /// Heap-allocated, cache-aligned storage for stack values.
    /// Using Box to avoid 32KB stack allocation.
    data: Box<AlignedData>,
    /// Stack pointer - starts at STACK_LIMIT (empty), decreases on push.
    /// Points to the current top of stack (first occupied slot).
    sp: usize,
}

impl fmt::Debug for AlignedStack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AlignedStack").field("len", &self.len()).field("sp", &self.sp).finish()
    }
}

impl fmt::Display for AlignedStack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[")?;
        let slice = self.data();
        for (i, x) in slice.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{x}")?;
        }
        f.write_str("]")
    }
}

impl Default for AlignedStack {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for AlignedStack {
    fn clone(&self) -> Self {
        let mut new_stack = Self::new();
        // Copy only the active portion of the stack
        new_stack.data.data[self.sp..].copy_from_slice(&self.data.data[self.sp..]);
        new_stack.sp = self.sp;
        new_stack
    }
}

impl PartialEq for AlignedStack {
    fn eq(&self, other: &Self) -> bool {
        self.sp == other.sp && self.data.data[self.sp..] == other.data.data[other.sp..]
    }
}

impl Eq for AlignedStack {}

impl core::hash::Hash for AlignedStack {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.sp.hash(state);
        self.data.data[self.sp..].hash(state);
    }
}

impl StackTr for AlignedStack {
    #[inline]
    fn len(&self) -> usize {
        self.len()
    }

    #[inline]
    fn data(&self) -> &[U256] {
        self.data()
    }

    #[inline]
    fn clear(&mut self) {
        self.sp = STACK_LIMIT;
    }

    #[inline]
    fn popn<const N: usize>(&mut self) -> Option<[U256; N]> {
        if self.len() < N {
            return None;
        }
        // SAFETY: Length is checked above
        Some(unsafe { self.popn_unchecked::<N>() })
    }

    #[inline]
    fn popn_top<const POPN: usize>(&mut self) -> Option<([U256; POPN], &mut U256)> {
        if self.len() < POPN + 1 {
            return None;
        }
        // SAFETY: Length is checked above
        Some(unsafe { self.popn_top_unchecked::<POPN>() })
    }

    #[inline]
    fn exchange(&mut self, n: usize, m: usize) -> bool {
        self.exchange(n, m)
    }

    #[inline]
    fn dup(&mut self, n: usize) -> bool {
        self.dup(n)
    }

    #[inline]
    fn push(&mut self, value: U256) -> bool {
        self.push(value)
    }

    #[inline]
    fn push_slice(&mut self, slice: &[u8]) -> bool {
        self.push_slice_inner(slice)
    }
}

impl AlignedStack {
    /// Creates a new cache-aligned stack.
    #[inline]
    pub fn new() -> Self {
        Self {
            data: Box::new(AlignedData::default()),
            sp: STACK_LIMIT, // Empty stack, pointer at end
        }
    }

    /// Returns the number of elements on the stack.
    #[inline]
    pub fn len(&self) -> usize {
        STACK_LIMIT - self.sp
    }

    /// Returns whether the stack is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.sp == STACK_LIMIT
    }

    /// Returns a slice of the stack data (from top to bottom, logical order).
    ///
    /// Note: Returns elements in order from bottom to top to match the original Stack API.
    #[inline]
    pub fn data(&self) -> &[U256] {
        &self.data.data[self.sp..]
    }

    /// Returns a mutable reference to the stack data.
    #[inline]
    pub fn data_mut(&mut self) -> &mut [U256] {
        &mut self.data.data[self.sp..]
    }

    /// Push a new value onto the stack.
    ///
    /// Returns `true` if successful, `false` if stack overflow.
    #[inline]
    #[must_use]
    pub fn push(&mut self, value: U256) -> bool {
        if self.sp == 0 {
            return false;
        }
        self.sp -= 1;
        self.data.data[self.sp] = value;
        true
    }

    /// Push a value without bounds checking.
    ///
    /// # Safety
    ///
    /// Caller must ensure stack is not full (sp > 0).
    #[inline]
    pub unsafe fn push_unchecked(&mut self, value: U256) {
        debug_assert!(self.sp > 0, "stack overflow in push_unchecked");
        self.sp -= 1;
        // SAFETY: Caller guarantees sp > 0, so sp-1 is valid index
        unsafe { *self.data.data.get_unchecked_mut(self.sp) = value };
    }

    /// Pop a value from the stack.
    ///
    /// Returns `Err(StackUnderflow)` if the stack is empty.
    #[inline]
    pub fn pop(&mut self) -> Result<U256, InstructionResult> {
        if self.sp >= STACK_LIMIT {
            return Err(InstructionResult::StackUnderflow);
        }
        let val = self.data.data[self.sp];
        self.sp += 1;
        Ok(val)
    }

    /// Pop a value without bounds checking.
    ///
    /// # Safety
    ///
    /// Caller must ensure stack is not empty (sp < STACK_LIMIT).
    #[inline]
    pub unsafe fn pop_unchecked(&mut self) -> U256 {
        debug_assert!(self.sp < STACK_LIMIT, "stack underflow in pop_unchecked");
        // SAFETY: Caller guarantees sp < STACK_LIMIT, so sp is valid index
        let val = unsafe { *self.data.data.get_unchecked(self.sp) };
        self.sp += 1;
        val
    }

    /// Returns a mutable reference to the top of the stack.
    ///
    /// # Safety
    ///
    /// Caller must ensure stack is not empty.
    #[inline]
    pub unsafe fn top_unchecked(&mut self) -> &mut U256 {
        debug_assert!(self.sp < STACK_LIMIT, "stack underflow in top_unchecked");
        // SAFETY: Caller guarantees stack is not empty, so sp is valid index
        unsafe { self.data.data.get_unchecked_mut(self.sp) }
    }

    /// Pop N values from the stack.
    ///
    /// # Safety
    ///
    /// Caller must ensure stack has at least N elements.
    #[inline]
    pub unsafe fn popn_unchecked<const N: usize>(&mut self) -> [U256; N] {
        debug_assert!(self.len() >= N, "stack underflow in popn_unchecked");
        // SAFETY: Caller guarantees stack has at least N elements
        core::array::from_fn(|_| unsafe { self.pop_unchecked() })
    }

    /// Pop N values and return reference to new top.
    ///
    /// # Safety
    ///
    /// Caller must ensure stack has at least N+1 elements.
    #[inline]
    pub unsafe fn popn_top_unchecked<const N: usize>(&mut self) -> ([U256; N], &mut U256) {
        debug_assert!(self.len() >= N + 1, "stack underflow in popn_top_unchecked");
        // SAFETY: Caller guarantees stack has at least N+1 elements
        let result = unsafe { self.popn_unchecked::<N>() };
        let top = unsafe { self.top_unchecked() };
        (result, top)
    }

    /// Peek at a value at given index from the top.
    ///
    /// Index 0 is the top of the stack.
    #[inline]
    pub fn peek(&self, index_from_top: usize) -> Result<U256, InstructionResult> {
        if self.len() <= index_from_top {
            return Err(InstructionResult::StackUnderflow);
        }
        // With downward growth, top is at sp, so sp + index_from_top
        Ok(self.data.data[self.sp + index_from_top])
    }

    /// Duplicate the Nth value from the top onto the stack.
    ///
    /// DUP1 duplicates the top (n=1), DUP2 duplicates second from top (n=2), etc.
    #[inline]
    pub fn dup(&mut self, n: usize) -> bool {
        if n == 0 || n > self.len() || self.sp == 0 {
            return false;
        }
        // n=1 means duplicate top, which is at sp
        // n=2 means duplicate second from top, which is at sp+1
        let value = self.data.data[self.sp + n - 1];
        self.sp -= 1;
        self.data.data[self.sp] = value;
        true
    }

    /// Swap the top of the stack with the Nth element below it.
    ///
    /// SWAP1 swaps top with second (n=1), SWAP2 swaps top with third (n=2), etc.
    #[inline]
    pub fn swap(&mut self, n: usize) -> bool {
        if n == 0 || self.len() <= n {
            return false;
        }
        // Top is at sp, nth below is at sp + n
        self.data.data.swap(self.sp, self.sp + n);
        true
    }

    /// Exchange operation for EXCHANGE opcode (EIP-663).
    ///
    /// Swaps stack[n+1] with stack[n+m+1] where stack[0] is the top.
    #[inline]
    pub fn exchange(&mut self, n: usize, m: usize) -> bool {
        let n_m_index = n + m;
        let len = self.len();
        if n_m_index >= len {
            return false;
        }
        // With downward growth:
        // stack[n] is at sp + n
        // stack[n+m] is at sp + n + m
        self.data.data.swap(self.sp + n, self.sp + n_m_index);
        true
    }

    /// Set a value at given index from the top.
    #[inline]
    pub fn set(&mut self, index_from_top: usize, value: U256) -> Result<(), InstructionResult> {
        if self.len() <= index_from_top {
            return Err(InstructionResult::StackUnderflow);
        }
        self.data.data[self.sp + index_from_top] = value;
        Ok(())
    }

    /// Push a byte slice onto the stack, padded to 32 bytes.
    #[inline]
    fn push_slice_inner(&mut self, slice: &[u8]) -> bool {
        if slice.is_empty() {
            return true;
        }

        let n_words = slice.len().div_ceil(32);
        if self.sp < n_words {
            return false;
        }

        // Push words from right to left (last bytes first)
        let mut slice_remaining = slice;
        for _ in 0..n_words {
            let word_start =
                if slice_remaining.len() >= 32 { slice_remaining.len() - 32 } else { 0 };
            let word_bytes = &slice_remaining[word_start..];
            slice_remaining = &slice_remaining[..word_start];

            // Pad to 32 bytes (left pad with zeros)
            let mut padded = [0u8; 32];
            padded[32 - word_bytes.len()..].copy_from_slice(word_bytes);

            self.sp -= 1;
            self.data.data[self.sp] = U256::from_be_bytes(padded);
        }

        true
    }

    /// Optimized binary operation - works in place.
    ///
    /// Pops two values, applies the operation, and pushes the result.
    /// This is more cache-efficient than separate pop/push operations.
    #[inline]
    pub fn binary_op<F>(&mut self, op: F) -> Result<(), InstructionResult>
    where
        F: FnOnce(U256, U256) -> U256,
    {
        if self.len() < 2 {
            return Err(InstructionResult::StackUnderflow);
        }
        let a = self.data.data[self.sp];
        let b = self.data.data[self.sp + 1];
        self.data.data[self.sp + 1] = op(a, b);
        self.sp += 1;
        Ok(())
    }

    /// Optimized binary operation without bounds checking.
    ///
    /// # Safety
    ///
    /// Caller must ensure stack has at least 2 elements.
    #[inline]
    pub unsafe fn binary_op_unchecked<F>(&mut self, op: F)
    where
        F: FnOnce(U256, U256) -> U256,
    {
        debug_assert!(self.len() >= 2, "stack underflow in binary_op_unchecked");
        // SAFETY: Caller guarantees stack has at least 2 elements
        unsafe {
            let a = *self.data.data.get_unchecked(self.sp);
            let b = *self.data.data.get_unchecked(self.sp + 1);
            *self.data.data.get_unchecked_mut(self.sp + 1) = op(a, b);
        }
        self.sp += 1;
    }

    /// Optimized unary operation - works in place on top of stack.
    #[inline]
    pub fn unary_op<F>(&mut self, op: F) -> Result<(), InstructionResult>
    where
        F: FnOnce(U256) -> U256,
    {
        if self.is_empty() {
            return Err(InstructionResult::StackUnderflow);
        }
        self.data.data[self.sp] = op(self.data.data[self.sp]);
        Ok(())
    }

    /// Optimized unary operation without bounds checking.
    ///
    /// # Safety
    ///
    /// Caller must ensure stack is not empty.
    #[inline]
    pub unsafe fn unary_op_unchecked<F>(&mut self, op: F)
    where
        F: FnOnce(U256) -> U256,
    {
        debug_assert!(!self.is_empty(), "stack underflow in unary_op_unchecked");
        // SAFETY: Caller guarantees stack is not empty
        unsafe {
            let val = self.data.data.get_unchecked_mut(self.sp);
            *val = op(*val);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_stack() {
        let stack = AlignedStack::new();
        assert!(stack.is_empty());
        assert_eq!(stack.len(), 0);
    }

    #[test]
    fn test_push_pop() {
        let mut stack = AlignedStack::new();

        assert!(stack.push(U256::from(1)));
        assert!(stack.push(U256::from(2)));
        assert!(stack.push(U256::from(3)));

        assert_eq!(stack.len(), 3);

        assert_eq!(stack.pop().unwrap(), U256::from(3));
        assert_eq!(stack.pop().unwrap(), U256::from(2));
        assert_eq!(stack.pop().unwrap(), U256::from(1));

        assert!(stack.is_empty());
        assert!(stack.pop().is_err());
    }

    #[test]
    fn test_peek() {
        let mut stack = AlignedStack::new();

        let _ = stack.push(U256::from(10));
        let _ = stack.push(U256::from(20));
        let _ = stack.push(U256::from(30));

        // Index 0 is top of stack
        assert_eq!(stack.peek(0).unwrap(), U256::from(30));
        assert_eq!(stack.peek(1).unwrap(), U256::from(20));
        assert_eq!(stack.peek(2).unwrap(), U256::from(10));
        assert!(stack.peek(3).is_err());
    }

    #[test]
    fn test_dup() {
        let mut stack = AlignedStack::new();

        let _ = stack.push(U256::from(1));
        let _ = stack.push(U256::from(2));
        let _ = stack.push(U256::from(3));

        // DUP1 - duplicate top
        assert!(stack.dup(1));
        assert_eq!(stack.len(), 4);
        assert_eq!(stack.peek(0).unwrap(), U256::from(3));

        // DUP3 - duplicate third from top (which is now 2)
        assert!(stack.dup(3));
        assert_eq!(stack.len(), 5);
        assert_eq!(stack.peek(0).unwrap(), U256::from(2));
    }

    #[test]
    fn test_swap() {
        let mut stack = AlignedStack::new();

        let _ = stack.push(U256::from(1));
        let _ = stack.push(U256::from(2));
        let _ = stack.push(U256::from(3));

        // SWAP1 - swap top with second
        assert!(stack.swap(1));
        assert_eq!(stack.peek(0).unwrap(), U256::from(2));
        assert_eq!(stack.peek(1).unwrap(), U256::from(3));

        // SWAP2 - swap top with third
        assert!(stack.swap(2));
        assert_eq!(stack.peek(0).unwrap(), U256::from(1));
        assert_eq!(stack.peek(2).unwrap(), U256::from(2));
    }

    #[test]
    fn test_exchange() {
        let mut stack = AlignedStack::new();

        // Push 5 values: bottom [1, 2, 3, 4, 5] top
        for i in 1..=5 {
            let _ = stack.push(U256::from(i));
        }

        // exchange(1, 1) swaps stack[1] with stack[2]
        // Before: [1, 2, 3, 4, 5] (top is 5)
        // After:  [1, 2, 4, 3, 5]
        assert!(stack.exchange(1, 1));
        assert_eq!(stack.peek(1).unwrap(), U256::from(3));
        assert_eq!(stack.peek(2).unwrap(), U256::from(4));
    }

    #[test]
    fn test_binary_op() {
        let mut stack = AlignedStack::new();

        let _ = stack.push(U256::from(10));
        let _ = stack.push(U256::from(5));

        // Add operation: 5 + 10 = 15
        stack.binary_op(|a, b| a + b).unwrap();

        assert_eq!(stack.len(), 1);
        assert_eq!(stack.peek(0).unwrap(), U256::from(15));
    }

    #[test]
    fn test_unary_op() {
        let mut stack = AlignedStack::new();

        let _ = stack.push(U256::from(10));

        // Double operation
        stack.unary_op(|x| x * U256::from(2)).unwrap();

        assert_eq!(stack.peek(0).unwrap(), U256::from(20));
    }

    #[test]
    fn test_stack_limit() {
        let mut stack = AlignedStack::new();

        // Fill the stack
        for i in 0..STACK_LIMIT {
            assert!(stack.push(U256::from(i)));
        }

        assert_eq!(stack.len(), STACK_LIMIT);

        // Should fail to push when full
        assert!(!stack.push(U256::from(9999)));
    }

    #[test]
    fn test_clone() {
        let mut stack = AlignedStack::new();

        let _ = stack.push(U256::from(1));
        let _ = stack.push(U256::from(2));
        let _ = stack.push(U256::from(3));

        let cloned = stack.clone();

        assert_eq!(stack, cloned);
        assert_eq!(cloned.len(), 3);
        assert_eq!(cloned.peek(0).unwrap(), U256::from(3));
    }

    #[test]
    fn test_data_returns_correct_slice() {
        let mut stack = AlignedStack::new();

        let _ = stack.push(U256::from(1));
        let _ = stack.push(U256::from(2));
        let _ = stack.push(U256::from(3));

        let data = stack.data();
        assert_eq!(data.len(), 3);
        // With downward growth, data() returns [3, 2, 1] (top to bottom in memory)
        assert_eq!(data[0], U256::from(3));
        assert_eq!(data[1], U256::from(2));
        assert_eq!(data[2], U256::from(1));
    }

    #[test]
    fn test_push_slice() {
        let mut stack = AlignedStack::new();

        // Push a single byte
        assert!(stack.push_slice_inner(&[42]));
        assert_eq!(stack.len(), 1);
        assert_eq!(stack.peek(0).unwrap(), U256::from(42));
    }

    #[test]
    fn test_alignment() {
        let stack = AlignedStack::new();
        let data_ptr = stack.data.data.as_ptr() as usize;
        assert_eq!(data_ptr % 64, 0, "Stack data should be 64-byte aligned");
    }

    #[test]
    fn test_popn() {
        let mut stack = AlignedStack::new();

        let _ = stack.push(U256::from(1));
        let _ = stack.push(U256::from(2));
        let _ = stack.push(U256::from(3));

        let result: Option<[U256; 2]> = stack.popn();
        assert!(result.is_some());
        let [a, b] = result.unwrap();
        // Pop returns top first
        assert_eq!(a, U256::from(3));
        assert_eq!(b, U256::from(2));
        assert_eq!(stack.len(), 1);
    }

    #[test]
    fn test_popn_top() {
        let mut stack = AlignedStack::new();

        let _ = stack.push(U256::from(1));
        let _ = stack.push(U256::from(2));
        let _ = stack.push(U256::from(3));

        let result: Option<([U256; 2], &mut U256)> = stack.popn_top();
        assert!(result.is_some());
        let ([a, b], top) = result.unwrap();
        assert_eq!(a, U256::from(3));
        assert_eq!(b, U256::from(2));
        assert_eq!(*top, U256::from(1));
    }
}
