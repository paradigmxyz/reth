bitflags::bitflags! {
    /// Marker to assign a transaction into a sub-pool:
    pub(crate) struct TxState: u8 {
        const ENOUGH_FEE_CAP_PROTOCOL = 0b100000;
        const NO_NONCE_GAPS = 0b010000;
        const ENOUGH_BALANCE = 0b001000;
        const NOT_TOO_MUCH_GAS = 0b000100;
        const ENOUGH_FEE_CAP_BLOCK = 0b000010;
        const IS_LOCAL = 0b000001;

        const BASE_FEE_POOL_BITS = TxState::ENOUGH_FEE_CAP_PROTOCOL.bits + TxState::NO_NONCE_GAPS.bits + TxState::ENOUGH_BALANCE.bits + TxState::NOT_TOO_MUCH_GAS.bits;
        const QUEUED_POOL_BITS  = TxState::ENOUGH_FEE_CAP_PROTOCOL.bits;
    }
}
