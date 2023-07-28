/// TX_SLOT_SIZE is used to calculate how many data slots a single transaction
/// takes up based on its size. The slots are used as DoS protection, ensuring
/// that validating a new transaction remains a constant operation (in reality
/// O(maxslots), where max slots are 4 currently).
pub const TX_SLOT_SIZE: usize = 32 * 1024;

/// TX_MAX_SIZE is the maximum size a single transaction can have. This field has
/// non-trivial consequences: larger transactions are significantly harder and
/// more expensive to propagate; larger transactions also take more resources
/// to validate whether they fit into the pool or not.
pub const TX_MAX_SIZE: usize = 4 * TX_SLOT_SIZE; // 128KB

/// Maximum bytecode to permit for a contract
pub const MAX_CODE_SIZE: usize = 24576;

/// Maximum initcode to permit in a creation transaction and create instructions
pub const MAX_INIT_CODE_SIZE: usize = 2 * MAX_CODE_SIZE;
