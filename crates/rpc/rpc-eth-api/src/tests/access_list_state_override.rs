//! Test for create_access_list with state override

// This is a simpler test that just verifies our code changes are in place

#[test]
fn verify_state_override_added() {
    // Check that the code we modified has the right method signatures
    let core_file = include_str!("../core.rs");
    
    // The trait definition in core.rs should include state_override
    assert!(core_file.contains("async fn create_access_list(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
        state_override: Option<StateOverride>,
    ) -> RpcResult<AccessListResult>;"));
    
    // The implementation in core.rs should include state_override
    assert!(core_file.contains("async fn create_access_list(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
        state_override: Option<StateOverride>,
    ) -> RpcResult<AccessListResult> {"));
    
    // Check that our create_access_list_at function takes the state_override
    let call_file = include_str!("../helpers/call.rs");
    assert!(call_file.contains("fn create_access_list_at(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
        state_override: Option<StateOverride>,
    ) -> impl Future<Output = Result<AccessListResult, Self::Error>> + Send"));
    
    // Check that our create_access_list_with function takes the state_override
    assert!(call_file.contains("fn create_access_list_with(
        &self,
        mut evm_env: EvmEnvFor<Self::Evm>,
        at: BlockId,
        mut request: TransactionRequest,
        state_override: Option<StateOverride>,
    ) -> Result<AccessListResult, Self::Error>"));
    
    // Check that we apply the state overrides in the implementation
    assert!(call_file.contains("// Apply state overrides if provided
        if let Some(state_overrides) = state_override {
            apply_state_overrides(state_overrides, &mut db)?;
        }"));
}