use thiserror::Error;

/// Error during [`EthBuiltPayload`](crate::EthBuiltPayload) to execution payload envelope
/// conversion.
#[derive(Error, Debug)]
pub enum BuiltPayloadConversionError {
    /// Unexpected EIP-4844 sidecars in the built payload.
    #[error("unexpected EIP-4844 sidecars")]
    UnexpectedEip4844Sidecars,
    /// Unexpected EIP-7594 sidecars in the built payload.
    #[error("unexpected EIP-7594 sidecars")]
    UnexpectedEip7594Sidecars,
}
