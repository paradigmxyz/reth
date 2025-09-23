/// Early validation macro that returns the block in case there was an error.
///
/// ```rust,ignore
/// fn process_block(&self, input: InputType) -> Result<Output, Error> {
///     let provider = ensure_ok!(self, input, self.state_provider_builder(hash, state));
///     // automatically converts block using self.convert_to_block(input)
/// }
/// ```
///
/// # Behavior
/// - **Success**: Returns `T` directly (zero-cost)
/// - **Error**: Automatically converts block using `self.convert_to_block(input)`
/// - **Requirements**: Must be called in a method with self parameter
#[macro_export]
macro_rules! ensure_ok {
    ($self:expr, $input:expr, $expr:expr) => {
        match $expr {
            Ok(val) => val,
            Err(e) => {
                let block = $self.convert_to_block($input)?;
                return Err($crate::tree::error::InsertBlockError::new(
                    block.into_sealed_block(),
                    e.into(),
                )
                .into())
            }
        }
    };
}
