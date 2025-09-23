/// Early validation macro that returns the block in case there was an error.
///
/// Handles `Result<T, E>` expressions efficiently by returning success values directly
/// and only using the pre-converted block when errors occur.
///
/// ```rust,ignore
/// fn process_with_pre_converted_block(&self, input: InputType) -> Result<Output, Error> {
///     let block = self.convert_to_block(&input)?;
///     let provider = ensure_ok!(self.state_provider_builder(hash, state), block);
///     // block is passed explicitly - no conversion needed on error
/// }
/// ```
///
/// # Behavior
/// - **Success**: Returns `T` directly (zero-cost)
/// - **Error**: Uses provided block with `InsertBlockError`
#[macro_export]
macro_rules! ensure_ok {
    ($expr:expr, $block:expr) => {
        match $expr {
            Ok(val) => val,
            Err(e) => {
                return Err($crate::tree::error::InsertBlockError::new(
                    $block.into_sealed_block(),
                    e.into(),
                )
                .into())
            }
        }
    };
}
