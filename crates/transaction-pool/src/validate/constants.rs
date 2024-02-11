/// [`TX_SLOT_BYTE_SIZE`] is used to calculate how many data slots a single transaction
/// takes up based on its byte size. The slots are used as DoS protection, ensuring
/// that validating a new transaction remains a constant operation (in reality
/// O(maxslots), where max slots are 4 currently).
pub const TX_SLOT_BYTE_SIZE: usize = 32 * 1024;

/// [`DEFAULT_MAX_TX_INPUT_BYTES`] is the default maximum size a single transaction can have. This
/// field has non-trivial consequences: larger transactions are significantly harder and
/// more expensive to propagate; larger transactions also take more resources
/// to validate whether they fit into the pool or not. Default is 4 times [`TX_SLOT_BYTE_SIZE`],
/// which defaults to 32 KiB, so 128 KiB.
pub const DEFAULT_MAX_TX_INPUT_BYTES: usize = 4 * TX_SLOT_BYTE_SIZE; // 128KB

/// Maximum bytecode to permit for a contract.
pub const MAX_CODE_BYTE_SIZE: usize = 24576;

/// Maximum initcode to permit in a creation transaction and create instructions.
pub const MAX_INIT_CODE_BYTE_SIZE: usize = 2 * MAX_CODE_BYTE_SIZE;
