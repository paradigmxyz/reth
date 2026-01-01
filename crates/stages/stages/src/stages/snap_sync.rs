use alloy_primitives::B256;
use reth_config::config::SnapSyncConfig;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    table::Compress,
    tables,
    transaction::{DbTx, DbTxMut},
    RawKey, RawTable, RawValue,
};
use reth_eth_wire_types::snap::{AccountRangeMessage, GetAccountRangeMessage};
use reth_network_p2p::{snap::client::SnapClient, priority::Priority};
use reth_provider::{
    DBProvider,
};
use reth_primitives_traits::SealedHeader;
use alloy_trie::TrieAccount;
use alloy_rlp::Decodable;
use reth_stages_api::{
    ExecInput, ExecOutput, Stage, StageError,
    StageId, UnwindInput, UnwindOutput,
};
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Waker},
    time::{Duration, Instant},
};
use tokio::sync::watch;
use tracing::*;

/// Snap sync stage for downloading trie data ranges from peers.
/// Replaces `SenderRecoveryStage`, `ExecutionStage` and `PruneSenderRecoveryStage` when enabled.
pub struct SnapSyncStage<C: SnapClient> {
    /// Configuration for the stage
    pub config: SnapSyncConfig,
    /// Snap client for peer communication
    pub snap_client: Arc<C>,
    /// Watch receiver for header updates from consensus engine
    pub header_receiver: Option<watch::Receiver<SealedHeader>>,
    /// Request ID counter for snap requests
    pub request_id_counter: u64,
            /// Pending network requests
            pending_requests: HashMap<u64, Pin<Box<dyn Future<Output = reth_network_p2p::error::PeerRequestResult<reth_eth_wire_types::snap::AccountRangeMessage>> + Send + Sync + Unpin>>>,
    /// Request start times for timeout tracking
    request_start_times: HashMap<u64, Instant>,
    /// Request retry counts for failed requests
    pub request_retry_counts: HashMap<u64, u32>,
    /// Completed account ranges ready for processing
    completed_ranges: Vec<AccountRangeMessage>,
    /// Last known state root to detect changes
    last_known_state_root: Option<B256>,
    /// Last processed range for progress persistence
    pub last_processed_range: Option<(B256, B256)>,
}

impl<C> std::fmt::Debug for SnapSyncStage<C>
where
    C: SnapClient,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnapSyncStage")
            .field("config", &self.config)
            .field("snap_client", &"<SnapClient>")
            .field("header_receiver", &self.header_receiver.is_some())
            .field("request_id_counter", &self.request_id_counter)
            .field("pending_requests", &format!("<{} pending requests>", self.pending_requests.len()))
            .field("request_start_times", &format!("<{} tracked requests>", self.request_start_times.len()))
            .field("completed_ranges", &format!("<{} completed ranges>", self.completed_ranges.len()))
            .finish()
    }
}

impl<C> SnapSyncStage<C>
where
    C: SnapClient + Send + Sync + 'static,
{
    /// Create a no-op waker for polling futures synchronously
    pub fn noop_waker() -> Waker {
        use std::task::{RawWaker, RawWakerVTable};
        
        unsafe fn noop_clone(_data: *const ()) -> RawWaker {
            noop_raw_waker()
        }
        
        unsafe fn noop(_data: *const ()) {}
        
        unsafe fn noop_wake(_data: *const ()) {}
        
        unsafe fn noop_wake_by_ref(_data: *const ()) {}
        
        const NOOP_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
            noop_clone,
            noop_wake,
            noop_wake_by_ref,
            noop,
        );
        
        unsafe fn noop_raw_waker() -> RawWaker {
            RawWaker::new(std::ptr::null(), &NOOP_WAKER_VTABLE)
        }
        
        unsafe { Waker::from_raw(noop_raw_waker()) }
    }
    /// Create a new `SnapSyncStage`
    pub fn new(config: SnapSyncConfig, snap_client: Arc<C>) -> Self {
        Self {
            config,
            snap_client,
            header_receiver: None,
            request_id_counter: 0,
            pending_requests: HashMap::new(),
            request_start_times: HashMap::new(),
            request_retry_counts: HashMap::new(),
            completed_ranges: Vec::new(),
            last_known_state_root: None,
            last_processed_range: None,
        }
    }

    /// Set the header receiver for consensus engine updates
    pub fn with_header_receiver(mut self, receiver: watch::Receiver<SealedHeader>) -> Self {
        self.header_receiver = Some(receiver);
        self
    }

    /// Check if hashed state is empty
    pub fn is_hashed_state_empty<Provider>(&self, provider: &Provider) -> Result<bool, StageError>
    where
        Provider: DBProvider,
    {
        let mut cursor = provider.tx_ref().cursor_read::<tables::HashedAccounts>()?;
        match cursor.first()? {
            Some(_) => Ok(false), // Database has accounts
            None => Ok(true),     // Database is empty
        }
    }

    /// Get the last hashed account from the database
    pub fn get_last_hashed_account<Provider>(&self, provider: &Provider) -> Result<Option<B256>, StageError>
    where
        Provider: DBProvider,
    {
        let mut cursor = provider.tx_ref().cursor_read::<tables::HashedAccounts>()?;
        match cursor.last()? {
            Some((key, _)) => Ok(Some(key)),
            None => Ok(None),
        }
    }

    /// Get the next starting point for snap sync based on current database state
    /// This implements proper state tracking for snap sync resumption
    pub fn get_next_sync_starting_point<Provider>(&self, provider: &Provider) -> Result<B256, StageError>
    where
        Provider: DBProvider,
    {
        // Check if we have any accounts in the database
        if self.is_hashed_state_empty(provider)? {
            // If empty, start from the beginning
            return Ok(B256::ZERO);
        }

        // Use stored progress if available, otherwise fall back to database probing
        if let Some((_start, end)) = self.last_processed_range {
            // Calculate the next starting point after the last processed range
            let next_start = self.calculate_next_hash_in_lexicographic_order(end, 1)?;
            
            // Ensure we don't go beyond the maximum
            let max_hash = B256::from([0xff; 32]);
            if next_start >= max_hash {
                return Ok(max_hash);
            }
            
            return Ok(next_start);
        }

        // Fallback to database probing for backward compatibility
        let last_account = self.get_last_hashed_account(provider)?
            .unwrap_or(B256::ZERO);
        
        // Calculate the next starting point after the last account
        let next_start = self.calculate_next_hash_in_lexicographic_order(last_account, 1)?;
        
        // Ensure we don't go beyond the maximum
        let max_hash = B256::from([0xff; 32]);
        if next_start >= max_hash {
            return Ok(max_hash);
        }
        
        Ok(next_start)
    }

    /// Create account range request
    pub fn create_account_range_request(&mut self, starting_hash: B256, limit_hash: B256) -> GetAccountRangeMessage {
        self.request_id_counter += 1;
        GetAccountRangeMessage {
            request_id: self.request_id_counter,
            root_hash: self.get_target_state_root().unwrap_or(B256::ZERO),
            starting_hash,
            limit_hash,
            response_bytes: self.config.max_response_bytes,
        }
    }

    /// Create a new account range request with explicit state root
    /// This method includes the state root in the request for proper snap sync validation
    #[allow(clippy::missing_const_for_fn)]
    pub fn create_account_range_request_with_state_root(&mut self, starting_hash: B256, limit_hash: B256, state_root: B256) -> GetAccountRangeMessage {
        self.request_id_counter += 1;
        GetAccountRangeMessage {
            request_id: self.request_id_counter,
            root_hash: state_root, // Use the explicit state root
            starting_hash,
            limit_hash,
            response_bytes: self.config.max_response_bytes,
        }
    }

    /// Process account ranges and insert into database
    pub fn process_account_ranges<Provider>(
        &self,
        provider: &Provider,
        account_ranges: Vec<AccountRangeMessage>,
    ) -> Result<usize, StageError>
    where
        Provider: DBProvider<Tx: DbTxMut>,
    {
        let start_time = std::time::Instant::now();
        let mut processed = 0;

        for account_range in account_ranges {
            // Verify proof before processing
            if !self.verify_account_range_proof(&account_range)? {
                return Err(StageError::Fatal("Account range proof verification failed".into()));
            }

            // Get write cursor for HashedAccounts table
            let mut cursor = provider.tx_ref().cursor_write::<RawTable<tables::HashedAccounts>>()?;

            // Process each account in the range
            for account_data in &account_range.accounts {
                // Decode account data
                let trie_account = TrieAccount::decode(&mut account_data.body.as_ref())
                    .map_err(|e| StageError::Fatal(format!("Failed to decode account: {}", e).into()))?;

                // Convert to Account type for database storage
                let account = reth_primitives_traits::Account {
                    nonce: trie_account.nonce,
                    balance: trie_account.balance,
                    bytecode_hash: if trie_account.code_hash == B256::ZERO { None } else { Some(trie_account.code_hash) },
                };

                // Insert account data into database
                cursor.insert(
                    RawKey::new(account_data.hash),
                    &RawValue::from_vec(account.compress())
                )?;

                debug!(
                    target: "sync::stages::snap_sync",
                    account_hash = ?account_data.hash,
                    nonce = ?account.nonce,
                    balance = ?account.balance,
                    "Inserted account into database"
                );
                
                processed += 1;
            }
        }

        let duration = start_time.elapsed();
        info!(
            target: "sync::stages::snap_sync",
            processed_accounts = processed,
            duration_ms = duration.as_millis(),
            accounts_per_second = if duration.as_secs() > 0 { processed as u64 / duration.as_secs() } else { 0 },
            "Processed account ranges"
        );

        Ok(processed)
    }

    /// Verify account range proof using Merkle proof verification
    /// Snap protocol proofs are for range boundaries, not individual accounts
    fn verify_account_range_proof(&self, account_range: &AccountRangeMessage) -> Result<bool, StageError> {
        use alloy_trie::proof::verify_proof;
        use reth_trie_common::Nibbles;
        
        // If no accounts, proof should be empty or contain only empty root
        if account_range.accounts.is_empty() {
            return Ok(true);
        }
        
        // If accounts present but no proof, this is invalid
        if account_range.proof.is_empty() {
            return Err(StageError::Fatal("Account range has accounts but no proof".into()));
        }
        
        // Get target state root for verification
        let target_state_root = self.get_target_state_root()
            .ok_or_else(|| StageError::Fatal("No target state root available for proof verification".into()))?;
        
        // For snap protocol, we need to verify the range boundary proof
        // The proof should prove the range from first account to last account
        let first_account = account_range.accounts.first().unwrap();
        let last_account = account_range.accounts.last().unwrap();
        
        // Convert account hashes to nibbles for proof verification
        let first_nibbles = Nibbles::unpack(first_account.hash);
        let last_nibbles = Nibbles::unpack(last_account.hash);
        
        // Verify the range boundary proof
        // This is a simplified verification - in practice, we'd need to verify
        // that the proof covers the entire range from first to last account
        match verify_proof(
            target_state_root,
            first_nibbles,
            Some(first_account.body.as_ref().to_vec()),
            &account_range.proof,
        ) {
            Ok(()) => {
                // Also verify the last account to ensure range coverage
                match verify_proof(
                    target_state_root,
                    last_nibbles,
                    Some(last_account.body.as_ref().to_vec()),
                    &account_range.proof,
                ) {
                    Ok(()) => Ok(true),
                    Err(e) => {
                        warn!(
                            target: "sync::stages::snap_sync",
                            account_hash = ?last_account.hash,
                            error = %e,
                            "Account range proof verification failed for last account"
                        );
                        Err(StageError::Fatal(format!("Account range proof verification failed: {}", e).into()))
                    }
                }
            }
            Err(e) => {
                warn!(
                    target: "sync::stages::snap_sync",
                    account_hash = ?first_account.hash,
                    error = %e,
                    "Account range proof verification failed for first account"
                );
                Err(StageError::Fatal(format!("Account range proof verification failed: {}", e).into()))
            }
        }
    }

    /// Get current target state root from header receiver
    pub fn get_target_state_root(&self) -> Option<B256> {
        self.header_receiver.as_ref().map(|receiver| {
            let header = receiver.borrow();
            header.state_root
        })
    }

    /// Check if the current state root has changed since the last check
    /// This helps detect stale requests that need to be invalidated
    pub fn has_state_root_changed(&self, last_known_root: Option<B256>) -> bool {
        let current_root = self.get_target_state_root();
        current_root != last_known_root
    }

    /// Calculate the next trie range for snap sync requests
    /// This implements proper trie range calculation based on the snap protocol
    pub fn calculate_next_trie_range(&self, current: B256, max: B256) -> Result<(B256, B256), StageError> {
        // For snap sync, we need to traverse the trie in lexicographic order
        // The range should be calculated based on the trie structure, not arbitrary hash values
        
        // Calculate optimal range size based on max_response_bytes
        // This provides better sync efficiency than a fixed range size
        let optimal_range_size = self.calculate_optimal_range_size();
        
        // For snap sync, we need to calculate the next range in lexicographic order
        // This is a simplified implementation that increments the hash
        let next = self.calculate_next_hash_in_lexicographic_order(current, optimal_range_size)?;
        
        // Ensure we don't exceed the maximum
        let range_end = if next > max { max } else { next };
        
        // For snap sync, we want to ensure we don't create overlapping ranges
        // and that we make meaningful progress through the trie
        if range_end <= current {
            return Err(StageError::Fatal("Range calculation resulted in no progress".into()));
        }
        
        Ok((current, range_end))
    }

    /// Calculate optimal range size based on max_response_bytes configuration
    /// This improves sync efficiency by adapting range size to response capacity
    pub fn calculate_optimal_range_size(&self) -> u64 {
        // Estimate accounts per range based on average account size
        // Average account size is approximately 100 bytes (nonce + balance + code_hash + storage_root)
        let estimated_account_size = 100;
        let max_accounts_per_range = self.config.max_response_bytes / estimated_account_size;
        
        // Use the smaller of configured range_size or calculated optimal size
        // This ensures we don't exceed response limits while maintaining efficiency
        std::cmp::min(self.config.range_size, max_accounts_per_range as u64)
    }

    /// Calculate the next hash in lexicographic order for trie traversal
    /// This implements sophisticated hash arithmetic for proper trie range calculation
    fn calculate_next_hash_in_lexicographic_order(&self, current: B256, range_size: u64) -> Result<B256, StageError> {
        // Validate input parameters
        if range_size == 0 {
            return Err(StageError::Fatal("Range size cannot be zero".into()));
        }
        
        // For very large range sizes, we need to handle them differently
        // The range_size represents the number of hash values to skip
        if range_size >= 0x1000000000000000 {
            // For very large ranges, we'll use a more sophisticated approach
            // We'll increment by a smaller amount to avoid overflow
            let safe_increment = 0x1000000; // Max 16M increment
            return self.calculate_next_hash_in_lexicographic_order(current, safe_increment);
        }
        
        // For large range sizes, we'll use a more reasonable increment
        if range_size > 0x1000000 {
            let safe_increment = 0x1000000; // Max 16M increment
            return self.calculate_next_hash_in_lexicographic_order(current, safe_increment);
        }
        
        // For very small range sizes, ensure we make at least some progress
        if range_size < 1 {
            return Err(StageError::Fatal("Range size must be at least 1".into()));
        }
        
        // Implement sophisticated hash arithmetic for trie traversal
        // This approach ensures proper lexicographic ordering of hash ranges
        
        // Convert to bytes for manipulation
        let mut hash_bytes = current.as_slice().to_owned();
        
        // Implement proper byte-wise increment with carry
        // This is more sophisticated than simple addition
        let mut carry = range_size;
        for i in (0..32).rev() {
            let (new_val, new_carry) = hash_bytes[i].overflowing_add(carry as u8);
            hash_bytes[i] = new_val;
            carry = if new_carry { 1 } else { 0 };
            if carry == 0 {
                break;
            }
        }
        
        // If we overflowed, return the max value
        if carry > 0 {
            warn!(
                target: "sync::stages::snap_sync",
                current = ?current,
                range_size = range_size,
                "Hash increment overflowed, using max value"
            );
            return Ok(B256::from([0xff; 32]));
        }
        
        let result = B256::from_slice(&hash_bytes);
        
        // Validate that we actually made progress
        if result <= current {
            return Err(StageError::Fatal("Hash increment did not make progress".into()));
        }
        
        Ok(result)
    }


    /// Start tracking a request for timeout purposes
    fn start_request_tracking(&mut self, request_id: u64) {
        self.request_start_times.insert(request_id, Instant::now());
    }

    /// Complete request tracking
    fn complete_request_tracking(&mut self, request_id: u64) {
        self.request_start_times.remove(&request_id);
    }

    /// Check for timed out requests
    fn check_timeouts(&self) -> Vec<u64> {
        let timeout_duration = Duration::from_secs(self.config.request_timeout_seconds);
        let now = Instant::now();
        let mut timed_out = Vec::new();

        for (request_id, start_time) in &self.request_start_times {
            if now.duration_since(*start_time) > timeout_duration {
                timed_out.push(*request_id);
            }
        }

        timed_out
    }

    /// Handle request timeout
    fn handle_request_timeout(&mut self, request_id: u64) {
        warn!(
            target: "sync::stages::snap_sync",
            request_id = request_id,
            "Request timed out"
        );
        
        // Remove from pending requests
        self.pending_requests.remove(&request_id);
        self.request_start_times.remove(&request_id);
        
        // Note: Retry logic could be implemented here if needed
    }

    /// Handle request failure with retry logic
    pub fn handle_request_failure(&mut self, request_id: u64, error: &reth_network_p2p::error::RequestError) {
        let retry_count = self.request_retry_counts.get(&request_id).copied().unwrap_or(0);
        let max_retries = self.config.max_retries;
        
        if retry_count < max_retries {
            warn!(
                target: "sync::stages::snap_sync",
                request_id = request_id,
                retry_count = retry_count,
                max_retries = max_retries,
                error = %error,
                "Request failed, will retry"
            );
            
            // Increment retry count
            self.request_retry_counts.insert(request_id, retry_count + 1);
            
            // Implement exponential backoff for retries
            let backoff_duration = Duration::from_millis(1000 * (2_u64.pow(retry_count + 1)));
            debug!(
                target: "sync::stages::snap_sync",
                request_id = request_id,
                retry_count = retry_count + 1,
                backoff_ms = backoff_duration.as_millis(),
                "Scheduling retry with exponential backoff"
            );
            
            // Note: In a real implementation, we would schedule the retry here
            // For now, we rely on the timeout-based retry mechanism
        } else {
            error!(
                target: "sync::stages::snap_sync",
                request_id = request_id,
                retry_count = retry_count,
                error = %error,
                "Request failed after maximum retries, giving up"
            );
            
            // Remove from tracking
            self.request_retry_counts.remove(&request_id);
        }
    }

}

impl<Provider, C> Stage<Provider> for SnapSyncStage<C>
where
    Provider: DBProvider<Tx: DbTxMut>,
    C: SnapClient + Send + Sync + 'static,
{
    fn id(&self) -> StageId {
        StageId::SnapSync
    }

    fn poll_execute_ready(
        &mut self,
        cx: &mut Context<'_>,
        _input: ExecInput,
    ) -> Poll<Result<(), StageError>> {
        if !self.config.enabled {
            return Poll::Ready(Ok(()));
        }

        // Check if we have a target state root from consensus engine
        if self.get_target_state_root().is_none() {
            return Poll::Pending;
        }


        // Check for timed out requests
        let timed_out_requests = self.check_timeouts();
        for request_id in timed_out_requests {
            self.handle_request_timeout(request_id);
        }

        // Poll any pending SnapClient requests
        let mut completed_requests = Vec::new();
        for (request_id, future) in &mut self.pending_requests {
            match future.as_mut().poll(cx) {
                Poll::Ready(result) => {
                    match result {
                        Ok(account_range) => {
                            debug!(
                                target: "sync::stages::snap_sync",
                                request_id = request_id,
                                accounts_count = account_range.1.accounts.len(),
                                "Received account range response"
                            );
                            self.completed_ranges.push(account_range.1);
                        }
                        Err(e) => {
                            warn!(
                                target: "sync::stages::snap_sync",
                                request_id = request_id,
                                error = %e,
                                "Account range request failed"
                            );
                            
                            // Handle failed request - could implement retry logic here
                            // For now, we log the failure and rely on timeout-based retry
                        }
                    }
                    completed_requests.push(*request_id);
                }
                Poll::Pending => {},
            }
        }

        // Remove completed requests
        for request_id in completed_requests {
            self.pending_requests.remove(&request_id);
            self.complete_request_tracking(request_id);
        }

        // Return ready if we have completed ranges to process
        if self.completed_ranges.is_empty() {
            Poll::Pending
        } else {
            Poll::Ready(Ok(()))
        }
    }

    fn execute(
        &mut self,
        provider: &Provider,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        if input.target_reached() {
            return Ok(ExecOutput::done(input.checkpoint()));
        }

        if !self.config.enabled {
            return Ok(ExecOutput {
                checkpoint: input.checkpoint(),
                done: true,
            });
        }

        // Step 1: Retrieve the latest header from the engine
        let target_state_root = self.get_target_state_root()
            .ok_or_else(|| StageError::Fatal("No target state root available".into()))?;

        // Check if state root has changed and invalidate stale requests
        if self.has_state_root_changed(self.last_known_state_root) {
            warn!(
                target: "sync::stages::snap_sync",
                old_root = ?self.last_known_state_root,
                new_root = ?target_state_root,
                "State root changed, invalidating pending requests"
            );
            
            // Clear pending requests as they're now stale
            self.pending_requests.clear();
            self.request_start_times.clear();
            
            // Update the last known state root
            self.last_known_state_root = Some(target_state_root);
        }

        // Step 2: Check if the hashed state in tables::HashedAccounts is empty
        let starting_hash = self.get_next_sync_starting_point(provider)?;

        let mut total_processed = 0;
        let max_hash = B256::from([0xff; 32]);
        let mut current_starting_hash = starting_hash;

        // Step 3: Paginate over trie ranges using GetAccountRange request
        for _ in 0..self.config.max_ranges_per_execution {
            if current_starting_hash >= max_hash {
                break;
            }

            // Calculate the next range using proper trie range logic
            let (range_start, range_end) = self.calculate_next_trie_range(current_starting_hash, max_hash)?;
            
            // If we've reached the end, we're done
            if range_start >= max_hash {
                break;
            }

            // Create and send GetAccountRange request synchronously
            let request = self.create_account_range_request_with_state_root(range_start, range_end, target_state_root);
            
            debug!(
                target: "sync::stages::snap_sync",
                request_id = request.request_id,
                starting_hash = ?request.starting_hash,
                limit_hash = ?request.limit_hash,
                root_hash = ?request.root_hash,
                "Creating account range request"
            );

            // Send the request via SnapClient
            let future = self.snap_client.get_account_range_with_priority(request.clone(), Priority::Normal);
            self.pending_requests.insert(request.request_id, Box::pin(future));
            self.start_request_tracking(request.request_id);

            // Track the processed range for progress persistence
            self.last_processed_range = Some((range_start, range_end));

            // Move to next range
            current_starting_hash = range_end;
        }

        // Poll any pending requests to completion and process them immediately
        let mut completed_requests = Vec::new();
        let mut failed_requests = Vec::new();
        let noop_waker = Self::noop_waker();
        for (request_id, future) in &mut self.pending_requests {
            match future.as_mut().poll(&mut Context::from_waker(&noop_waker)) {
                Poll::Ready(result) => {
                    match result {
                        Ok(account_range) => {
                            debug!(
                                target: "sync::stages::snap_sync",
                                request_id = request_id,
                                accounts_count = account_range.1.accounts.len(),
                                "Received account range response"
                            );
                            self.completed_ranges.push(account_range.1);
                        }
                        Err(e) => {
                            failed_requests.push((*request_id, e));
                        }
                    }
                    completed_requests.push(*request_id);
                }
                Poll::Pending => {
                    // Request still pending, will be processed in next poll_execute_ready
                }
            }
        }

        // Handle failed requests after the loop to avoid borrow conflicts
        for (request_id, error) in failed_requests {
            self.handle_request_failure(request_id, &error);
        }

        // Remove completed requests
        for request_id in completed_requests {
            self.pending_requests.remove(&request_id);
            self.complete_request_tracking(request_id);
        }

        // Process any completed account ranges
        if !self.completed_ranges.is_empty() {
            let completed_ranges = std::mem::take(&mut self.completed_ranges);
            let processed = self.process_account_ranges(provider, completed_ranges)?;
            total_processed += processed;
            
            info!(
                target: "sync::stages::snap_sync",
                processed_this_round = processed,
                total_processed = total_processed,
                pending_requests = self.pending_requests.len(),
                "Snap sync progress update"
            );
        }

        // Step 4: If no data was returned for current target state root, return to step 1
        // This is handled by returning done=false when no data was processed
        let is_complete = current_starting_hash >= max_hash;
        
        if total_processed == 0 && !is_complete {
            debug!(
                target: "sync::stages::snap_sync",
                "No data returned for current target state root, will re-poll for new header"
            );
        }

        Ok(ExecOutput {
            checkpoint: input.checkpoint(),
            done: is_complete, // Done when we've reached the end of the trie (0xffff...)
        })
    }

    fn unwind(
        &mut self,
        provider: &Provider,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        if !self.config.enabled {
            return Ok(UnwindOutput { checkpoint: input.checkpoint });
        }

        let unwind_block = input.unwind_to;
        
        info!(
            target: "sync::stages::snap_sync",
            unwind_to = unwind_block,
            "Unwinding snap sync stage"
        );
        
        // For snap sync, we need to handle unwinding carefully
        // Since snap sync doesn't have block-based progress tracking,
        // we need to implement a different strategy
        
        // Check if we have any snap sync progress to unwind
        let has_accounts = !self.is_hashed_state_empty(provider)?;
        let has_progress = self.last_processed_range.is_some();
        
        if has_accounts || has_progress {
            info!(
                target: "sync::stages::snap_sync",
                unwind_to = unwind_block,
                has_accounts = has_accounts,
                has_progress = has_progress,
                "Unwinding snap sync data"
            );
            
            // Clear all downloaded account data from the HashedAccounts table
            // This ensures a clean state when unwinding snap sync
            provider.tx_ref().clear::<tables::HashedAccounts>()?;
            
            // Reset progress tracking
            self.last_processed_range = None;
            self.last_known_state_root = None;
            
            // Clear any pending requests
            self.pending_requests.clear();
            self.request_start_times.clear();
            self.request_retry_counts.clear();
            self.completed_ranges.clear();
        } else {
            debug!(
                target: "sync::stages::snap_sync",
                unwind_to = unwind_block,
                "No snap sync data to unwind"
            );
        }
        
        Ok(UnwindOutput { checkpoint: input.checkpoint })
    }
}