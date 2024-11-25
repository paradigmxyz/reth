# Tracing targets

Here instrumented functions are listed as well as events emitted in functions.

### Crates

#### reth-blockchain-tree

Targets:

1. **blockchain_tree**
    [Instrumented ***try_insert_validated_block*** fn with ***trace*** level](crates/blockchain-tree/src/blockchain_tree.rs#L317)
    [Instrumented ***try_append_canonical_chain*** fn with ***trace*** level](crates/blockchain-tree/src/blockchain_tree.rs#L382)
    [Instrumented ***try_append_canonical_chain*** fn with ***trace*** level. ***debug*** event is emitted](crates/blockchain-tree/src/blockchain_tree.rs#L382)
    [Instrumented ***try_insert_block_into_side_chain*** fn with ***trace*** level. ***debug*** event is emitted](crates/blockchain-tree/src/blockchain_tree.rs#L447)
    [Instrumented ***make_canonical*** fn with ***trace*** level. Emits ***info, trace, debug, error*** event](crates/blockchain-tree/src/blockchain_tree.rs#L1019)
    [***post_state_data*** fn emits events at ***trace, debug*** levels](crates/blockchain-tree/src/blockchain_tree.rs#L266)
    [***find_all_dependent_chains*** fn emits events at ***debug*** level](crates/blockchain-tree/src/blockchain_tree.rs#L586)
    [***insert_unwound_chain*** fn emits events at ***debug*** level](crates/blockchain-tree/src/blockchain_tree.rs#L623)
    [***validate_block*** fn emits events at ***error*** level](crates/blockchain-tree/src/blockchain_tree.rs#L696)
    [***is_block_inside_sidechain*** fn emits event at ***debug*** level](crates/blockchain-tree/src/blockchain_tree.rs#L727)
    [***try_connect_buffered_blocks*** fn emits events at ***trace, debug*** levels](crates/blockchain-tree/src/blockchain_tree.rs#L920)
    [***remove_and_split_chain*** fn emits events at ***trace*** levels](crates/blockchain-tree/src/blockchain_tree.rs#L942)    
    [***commit_canonical_to_database*** fn emits events at ***debug*** levels](crates/blockchain-tree/src/blockchain_tree.rs#L1211)
    [***revert_canonical_from_database*** fn emits events at ***trace, info*** levels](crates/blockchain-tree/src/blockchain_tree.rs#L1301)

2. **reth-chain-state**
    [***poll_next*** fn emits ***debug*** event](crates/chain-state/src/notifications.rs#L183)

3. **reth-cli-commands**
    [***execute*** fn emits ***debug*** event](crates/cli/commands/src/db/checksum.rs#L39)
    [***view*** fn emits ***info*** events](crates/cli/commands/src/db/checksum.rs#L70)
    [***Command::execute*** fn emits ***warn*** event](crates/cli/commands/src/db/diff.rs#L55)
    [***find_diffs*** fn emits ***info*** events](crates/cli/commands/src/db/diff.rs#L91)
    [***Command::execute*** fn emits ***error*** event](crates/cli/commands/src/db/get.rs#L56)
    [***ListTableViewer::view*** fn emits ***error*** event](crates/cli/commands/src/db/list.rs#L97)
    [***DBList::run*** fn emits ***error*** event](crates/cli/commands/src/db/tui.rs#L212)
    [***setup_without_evm*** fn emits ***info*** events](crates/cli/commands/src/init_state/without_evm.rs#L27)
    [***InitStateCommand::execute*** fn emits ***info*** events](crates/cli/commands/src/init_state/mod.rs#L71)
    [***Command::execute*** fn emits ***info*** events](crates/cli/commands/src/init_state/mod.rs#L71)
    [***dry_run*** fn emits ***info*** events](crates/cli/commands/src/stage/dump/execution.rs#L164)
    [***dry_run*** fn emits ***info*** events](crates/cli/commands/src/stage/dump/hashing_account.rs#L79)
    [***dry_run*** fn emits ***info*** events](crates/cli/commands/src/stage/dump/hashing_storage.rs#L74)
    [***dry_run*** fn emits ***info*** events](crates/cli/commands/src/stage/dump/merkle.rs#L149)
    [***setup*** fn emits ***info*** event](crates/cli/commands/src/stage/dump/mod.rs#L118)
    [***Command::execute*** fn emits ***info*** events](crates/cli/commands/src/stage/unwind.rs#L53)
    [***Command::execute*** fn emits ***info*** events](crates/cli/commands/src/stage/run.rs#L107)
    [***EnvironmentArgs::init*** fn emits ***info, debug*** events](crates/cli/commands/src/common.rs#L56)
    [***EnvironmentArgs::init*** fn emits ***info, warn*** events](crates/cli/commands/src/common.rs#L108)
    [***ImportCommand::execute*** fn emits ***info, debug, error*** events](crates/cli/commands/src/import.rs#L61)
    [***InitCommand::execute*** fn emits ***info*** events](crates/cli/commands/src/import.rs#L61)
    [***NodeCommand::execute*** fn emits ***info*** events](crates/cli/commands/src/node.rs#L140)
    [***PruneCommand::execute*** fn emits ***info*** events](crates/cli/commands/src/prune.rs#L20)
    [***CliRunner::run_command_until_exit*** fn emits ***debug, error*** events](crates/cli/runner/src/lib.rs#L32)
    [***CliRunner::run_blocking_until_ctrl_c*** fn emits ***trace*** events](crates/cli/runner/src/lib.rs#L172)

4. **reth-auto-seal-consensus**
    [***AutoSealClient::fetch_headers*** fn emits ***trace, warn*** events](crates/consensus/auto-seal/src/client.rs#L31)
    [***AutoSealClient::fetch_bodies*** fn emits ***trace*** events](crates/consensus/auto-seal/src/client.rs#L71)
    [***AutoSealClient::report_bad_message*** fn emits ***warn*** events](crates/consensus/auto-seal/src/client.rs#L122)
    [***MiningTask::poll*** fn emits ***warn, debug, error*** events](crates/consensus/auto-seal/src/task.rs#l93)
    [***StorageInner::insert_new_block*** fn emits ***trace*** events](crates/consensus/auto-seal/src/lib.rs#L253)
    [***StorageInner::build_and_execute*** fn emits ***trace*** events](crates/consensus/auto-seal/src/lib.rs#L333)

5. **reth-beacon-consensus**
    [Instrumented ***BeaconConsensusEngine::on_new_payload*** fn with ***trace*** level. Fn emits error event.](crates/consensus/beacon/src/engine/mod.rs#L1087)
    [Instrumented ***BeaconConsensusEngine::try_buffer_payload*** fn with ***trace*** level.](crates/consensus/beacon/src/engine/mod.rs#L1223)
    [Instrumented ***BeaconConsensusEngine::try_insert_new_payload*** fn with ***trace*** level.](crates/consensus/beacon/src/engine/mod.rs#L1223)
    [***InvalidHeaderCache::insert_with_invalid_ancestor*** fn emits ***warn*** event](crates/consensus/beacon/src/engine/invalid_headers.rs#L55)
    [***InvalidHeaderCache::insert*** fn emits ***warn*** event](crates/consensus/beacon/src/engine/invalid_headers.rs#L71)
    [***EngineSyncController::download_block_range*** fn emits ***warn*** event](crates/consensus/beacon/src/engine/sync.rs#L146)
    [***EngineSyncController::download_block_range*** fn emits ***trace*** event](crates/consensus/beacon/src/engine/sync.rs#L146)
    [***EngineSyncController::download_full_block*** fn emits ***trace*** event](crates/consensus/beacon/src/engine/sync.rs#L176)
    [***EngineSyncController::set_pipeline_sync_target*** fn emits ***trace*** event](crates/consensus/beacon/src/engine/sync.rs#L205)
    [***EngineSyncController::has_reached_max_block*** fn emits ***trace*** event](crates/consensus/beacon/src/engine/sync.rs#L220)
    [***EngineSyncController::poll*** fn emits ***trace*** events](crates/consensus/beacon/src/engine/sync.rs#L288)
    [***BeaconConsensusEngine::pre_validate_forkchoice_update*** fn emits ***trace*** events](crates/consensus/beacon/src/engine/mod.rs#L363)
    [***BeaconConsensusEngine::on_forkchoice_updated_make_canonical_result*** fn emits ***trace, debug, error*** events](crates/consensus/beacon/src/engine/mod.rs#L393)
    [***BeaconConsensusEngine::on_head_already_canonical*** fn emits ***debug*** events](crates/consensus/beacon/src/engine/mod.rs#L461)
    [***BeaconConsensusEngine::on_forkchoice_updated*** fn emits ***trace, warn*** events](crates/consensus/beacon/src/engine/mod.rs#L506)
    [***BeaconConsensusEngine::check_pipeline_consistency*** fn emits ***debug*** events](crates/consensus/beacon/src/engine/mod.rs#L595)
    [***BeaconConsensusEngine::can_pipeline_sync_to_finalized*** fn emits ***warn, debug*** events](crates/consensus/beacon/src/engine/mod.rs#L650)
    [***BeaconConsensusEngine::on_failed_canonical_forkchoice_update*** fn emits ***trace, warn, debug*** events](crates/consensus/beacon/src/engine/mod.rs#L986)
    [***BeaconConsensusEngine::try_make_sync_target_canonical*** fn emits ***debug*** events](crates/consensus/beacon/src/engine/mod.rs#L1338)
    [***BeaconConsensusEngine::on_sync_event*** fn emits ***trace, error*** events](crates/consensus/beacon/src/engine/mod.rs#L1401)
    [***BeaconConsensusEngine::on_pipeline_outcome*** fn emits ***warn, error*** events](crates/consensus/beacon/src/engine/mod.rs#L1451)
    [***BeaconConsensusEngine::set_canonical_head*** fn emits ***error*** event](crates/consensus/beacon/src/engine/mod.rs#L1551)
    [***BeaconConsensusEngine::on_hook_result*** fn emits ***error*** events](crates/consensus/beacon/src/engine/mod.rs#L1562)
    [***BeaconConsensusEngine::on_blockchain_tree_action*** fn emits ***trace, warn, debug, error*** events](crates/consensus/beacon/src/engine/mod.rs#L1604)
    [***EtherscanBlockProvider::subscribe_blocks*** fn emits ***warn*** events](crates/consensus/debug-client/src/providers/etherscan.rs#L54)
    [***DebugConsensusClient::run*** fn emits ***warn*** events](crates/consensus/debug-client/src/client.rs#L74)

6. **reth-e2e-test-utils**
    [***InvalidBlockWitnessHook::on_invalid_block*** fn emits ***info*** events](crates/engine/invalid-block-hooks/src/witness.rs#L60)
    [***InvalidBlockWitnessHook::on_invalid_block*** fn emits ***info*** event](crates/engine/invalid-block-hooks/src/witness.rs#L307)

7. **reth-engine-local**
    [***LocalMiner::run*** fn emits ***error*** events](crates/engine/local/src/miner.rs#L128)

8. **reth-engine-local**
    [***PersistenceState::schedule_removal*** fn emits ***debug*** event](crates/engine/tree/src/tree/persistence_state.rs#L36)
    [***PersistenceState::schedule_removal*** fn emits ***trace*** event](crates/engine/tree/src/tree/persistence_state.rs#L42)
    [***PipelineSync::set_pipeline_sync_target*** fn emits ***trace*** event](crates/engine/tree/src/backfill.rs#L120)

9. **reth-engine-tree**
    [Instrumented ***ChainOrchestrator::poll_next_event*** fn at ***debug*** level. Fn emits ***debug, error*** events.](crates/engine/tree/src/chain.rs#L75)
    [***BasicBlockDownloader::download_block_range*** fn emits ***trace*** event](crates/engine/tree/src/download.rs#L113)
    [***BasicBlockDownloader::download_full_block*** fn emits ***trace*** event](crates/engine/tree/src/download.rs#L137)
    [***BasicBlockDownloader::poll*** fn emits ***trace*** event](crates/engine/tree/src/download.rs#L196)
    [***PersistenceService::prune_before*** fn emits ***debug*** event](crates/engine/tree/src/persistence.rs#L53)
    [***PersistenceService::on_remove_blocks_above*** fn emits ***debug*** event](crates/engine/tree/src/persistence.rs#L112)
    [***PersistenceService::on_save_blocks*** fn emits ***debug*** event](crates/engine/tree/src/persistence.rs#L130)
    [***PersistenceHandle::spawn_service*** fn emits ***error*** event](crates/engine/tree/src/persistence.rs#L201)

10. **reth-engine-util**
    [***EngineMessageStore::engine_messages_iter*** fn emits ***warn, debug*** event](crates/engine/util/src/engine_store.rs#L101)
    [***EngineStoreStream::poll_next*** fn emits ***error*** event](crates/engine/util/src/reorg.rs#L116)
    [***EngineReorg::poll_next*** fn emits ***debug, error*** event](crates/engine/util/src/engine_store.rs#L147)
    [***create_reorg_head*** fn emits ***trace, debug*** events](crates/engine/util/src/reorg.rs#L248)
    [***EngineSkipFcu::poll_next*** fn emits ***warn*** event](crates/engine/util/src/skip_fcu.rs#L42)
    [***EngineSkipNewPayload::poll_next*** fn emits ***warn*** event](crates/engine/util/src/skip_new_payload.rs#L38)

11. **reth-ethereum-consensus**
    [***validate_block_post_execution*** fn emits ***debug*** event](crates/ethereum/consensus/src/validation.rs#L11)

12. **reth-node-ethereum**
    [***EthereumPoolBuilder::build_pool*** fn emits ***info, debug*** events](crates/ethereum/node/src/node.rs#L174)
    [***EthereumNetworkBuilder::build_network*** fn emits ***info*** event](crates/ethereum/node/src/node.rs#L312)

13. **reth-ethereum-payload-builder**
    [***default_ethereum_payload*** fn emits ***trace, warn, debug*** events](crates/ethereum/payload/src/lib.rs#L145)

14. **reth-exex**
    [Instrumented ***StreamBackfillJob::read_notification*** fn emits ***debug*** event](crates/exex/exex/src/wal/storage.rs#L124)
    [Instrumented ***StreamBackfillJob::write_notification*** fn emits ***debug*** event](crates/exex/exex/src/wal/storage.rs#L153)
    [Instrumented ***Storage::remove_notification*** fn emits ***debug*** events](crates/exex/exex/src/wal/storage.rs#L50)
    [***BackfillJob::execute_range*** fn emits ***trace, debug*** events](crates/exex/exex/src/backfill/job.rs#L68)
    [***SingleBlockBackfillJob::execute_block*** fn emits ***trace*** event](crates/exex/exex/src/backfill/job.rs#L189)
    [***StreamBackfillJob::poll_next*** fn emits ***debug*** event](crates/exex/exex/src/backfill/stream.rs#L110)
    [***ExExNotificationsWithHead::check_canonical*** fn emits ***debug*** events](crates/exex/exex/src/notifications.rs#L273)
    [***ExExNotificationsWithHead::check_backfill*** fn emits ***debug*** events](crates/exex/exex/src/notifications.rs#L316)
    [***ExExNotificationsWithHead::poll_next*** fn emits ***debug*** events](crates/exex/exex/src/notifications.rs#L347)

15. **reth-discv4**
    [***MockDiscovery::poll_next*** fn emits ***debug*** event](crates/net/discv4/src/test_utils.rs#L132)

16. **reth-discv5**
    [***default_ethereum_payload*** fn emits ***warn*** events](crates/net/discv5/src/config.rs#L402)

17. **reth-discv5**
    [***Discv5::set_eip868_in_local_enr*** fn emits ***error*** events](crates/net/discv5/src/lib.rs#L96)
    [***Discv5::ban*** fn emits ***error*** event](crates/net/discv5/src/lib.rs#L128)
    [***Discv5::start*** fn emits ***trace*** event](crates/net/discv5/src/lib.rs#L161)
    [***Discv5::on_discv5_update*** fn emits ***trace*** event](crates/net/discv5/src/lib.rs#L227)
    [***Discv5::on_discovered_peer*** fn emits ***trace*** events](crates/net/discv5/src/lib.rs#L289)
    [***bootstrap*** fn emits ***trace, debug*** events](crates/net/discv5/src/lib.rs#L490)
    [***spawn_populate_kbuckets_bg*** fn emits ***trace*** events](crates/net/discv5/src/lib.rs#L527)
    [***lookup*** fn emits ***trace, debug*** events](crates/net/discv5/src/lib.rs#L620)

18. **reth-dns-discovery**
    [***AsyncResolver::lookup_txt*** fn emits ***trace*** event ](crates/net/dns/src/resolver.rs#L16)

19. **reth-downloaders**
    [***BodiesDownloader::set_download_range*** fn emits ***trace, info, error*** events](crates/net/downloaders/src/bodies/bodies.rs#L310)
    [***BodiesDownloader::poll_next*** fn emits ***debug, error*** events](crates/net/downloaders/src/bodies/bodies.rs#L354)
    [***BodiesDownloader::on_error*** fn emits ***debug*** events](crates/net/downloaders/src/bodies/request.rs#L89)
    [***BodiesDownloader::submit_request*** fn emits ***trace*** event](crates/net/downloaders/src/bodies/request.rs#L109)
    [***BodiesDownloader::on_block_response*** fn emits ***trace*** event](crates/net/downloaders/src/bodies/request.rs#L118)
    [***SpawnedDownloader::poll*** fn emits ***trace*** event](crates/net/downloaders/src/bodies/request.rs#L118)
    [***SpawnedDownloader::poll*** fn emits ***trace*** event](crates/net/downloaders/src/bodies/task.rs#L114)
    [***ReverseHeadersDownloader::process_next_headers*** fn emits ***trace, error*** event](crates/net/downloaders/src/headers/reverse_headers.rs#L244)
    [***ReverseHeadersDownloader::on_sync_target_outcome*** fn emits ***trace*** event](crates/net/downloaders/src/headers/reverse_headers.rs#L361)
    [***ReverseHeadersDownloader::on_headers_outcome*** fn emits ***trace*** events](crates/net/downloaders/src/headers/reverse_headers.rs#L440)
    [***ReverseHeadersDownloader::penalize_peer*** fn emits ***trace*** events](crates/net/downloaders/src/headers/reverse_headers.rs#L526)
    [***ReverseHeadersDownloader::submit_request*** fn emits ***trace*** events](crates/net/downloaders/src/headers/reverse_headers.rs#L582)
    [***ReverseHeadersDownloader::update_sync_target*** fn emits ***trace*** events](crates/net/downloaders/src/headers/reverse_headers.rs#L675)
    [***ReverseHeadersDownloader::poll_next*** fn emits ***trace*** events](crates/net/downloaders/src/headers/reverse_headers.rs#L747)
    [***FileClient::from_reader*** fn emits ***trace*** events](crates/net/downloaders/src/file_client.rs#L192)
    [***FileClient::get_headers_with_priority*** fn emits ***trace, warn*** events](crates/net/downloaders/src/file_client.rs#L270)
    [***ChunkedFileReader::read_next_chunk*** fn emits ***debug*** events](crates/net/downloaders/src/file_client.rs#L412)
    [***ReceiptFileClient::from_receipt_reader*** fn emits ***trace, warn*** events](crates/net/downloaders/src/receipt_file_client.rs#L59)

20. **reth-ecies**
    [Instrumented fn ***ECIES::read_auth***](crates/net/ecies/src/algorithm.rs#L502)
    [Instrumented fn ***ECIES::read_ack***](crates/net/ecies/src/algorithm.rs#L576)
    [Instrumented fn ***ECIESState::decode*** at ***trace*** level. It also emits ***trace*** events](crates/net/ecies/src/codec.rs#L62)
    [Instrumented fn ***ECIESCodec::encode*** at ***trace*** level](crates/net/ecies/src/codec.rs#L154)
    [Instrumented fn ***ECIESStream::connect***](crates/net/ecies/src/stream.rs#L44)
    [***ECIESStream::connect_without_timeout*** fn emits ***trace*** events](crates/net/ecies/src/stream.rs#L65)
    [***ECIESStream::incoming*** fn emits ***trace*** events](crates/net/ecies/src/stream.rs#L100)

21. **reth-eth-wire**
    [***UnauthedEthStream::handshake_without_timeout*** fn emits ***trace, debug*** events](crates/net/eth-wire/src/ethstream.rs#L79)
    [***EthStream::poll_next*** fn emits ***debug*** event](crates/net/eth-wire/src/ethstream.rs#L264)
    [***UnauthedP2PStream::handshake*** fn emits ***trace, debug*** events](crates/net/eth-wire/src/p2pstream.rs#L89)
    [***UnauthedP2PStream::send_disconnect*** fn emits ***trace*** event](crates/net/eth-wire/src/p2pstream.rs#L178)
    [***P2PStream::start_disconnect*** fn emits ***debug*** event](crates/net/eth-wire/src/p2pstream.rs#L330)
    [***P2PStream::poll_next*** fn emits ***trace, debug*** events](crates/net/eth-wire/src/p2pstream.rs#L388)
    [***P2PStream::start_send*** fn emits ***debug*** event](crates/net/eth-wire/src/p2pstream.rs#L571)

22. **reth-net-nat**
    [***external_addr_with*** fn emits ***debug*** event](crates/net/nat/src/lib.rs#L198)
    [***external_addr_with*** fn emits ***debug*** event](crates/net/nat/src/lib.rs#L214)

23. **reth-network**
    [Instrumented fn ***start_pending_outbound_session***](crates/net/network/src/session/mod.rs#L857)
    [***ActiveSession::on_internal_peer_message*** fn emits ***debug*** event](crates/net/network/src/session/active.rs#L257)
    [***ActiveSession::handle_outgoing_response*** fn emits ***debug*** event](crates/net/network/src/session/active.rs#L295)
    [***ActiveSession::try_emit_broadcast*** fn emits ***trace*** event](crates/net/network/src/session/active.rs#L310)
    [***ActiveSession::try_emit_request*** fn emits ***trace*** event](crates/net/network/src/session/active.rs#L336)
    [***ActiveSession::emit_disconnect*** fn emits ***trace*** event](crates/net/network/src/session/active.rs#L368)
    [***ActiveSession::try_disconnect*** fn emits ***debug*** event](crates/net/network/src/session/active.rs#L409)
    [***ActiveSession::check_timed_out_requests*** fn emits ***debug*** event](crates/net/network/src/session/active.rs#L431)
    [***ActiveSession::poll*** fn emits ***trace, debug*** events](crates/net/network/src/session/active.rs#L479)
    [***SessionManager::on_incoming*** fn emits ***trace*** event](crates/net/network/src/session/mod.rs#L230)
    [***SessionManager::send_message*** fn emits ***debug*** event](crates/net/network/src/session/mod.rs#L351)
    [***SessionManager::try_disconnect_incoming_connection*** fn emits ***trace*** event](crates/net/network/src/session/mod.rs#L385)
    [***SessionManager::poll*** fn emits ***trace*** events](crates/net/network/src/session/mod.rs#L414)
    [***pending_session_with_timeout*** fn emits ***debug*** event](crates/net/network/src/session/mod.rs#L800)
    [***TransactionFetcher::buffer_hashes*** fn emits ***trace*** event](crates/net/network/src/transactions/fetcher.rs#L388)
    [***TransactionFetcher::on_fetch_pending_hashes*** fn emits ***trace*** events](crates/net/network/src/transactions/fetcher.rs#L432)
    [***TransactionFetcher::request_transactions_from_peer*** fn emits ***trace, debug*** events](crates/net/network/src/transactions/fetcher.rs#L634)
    [***TransactionFetcher::search_breadth_budget_find_idle_fallback_peer*** fn emits ***trace*** event](crates/net/network/src/transactions/fetcher.rs#L819)
    [***TransactionFetcher::search_breadth_budget_find_intersection_pending_hashes_and_hashes_seen_by_peer*** fn emits ***trace*** event](crates/net/network/src/transactions/fetcher.rs#L858)
    [***TransactionFetcher::on_resolved_get_pooled_transactions_request_fut*** fn emits ***trace*** events](crates/net/network/src/transactions/fetcher.rs#L907)
    [***UnverifiedPooledTransactions::verify*** fn emits ***trace*** events](crates/net/network/src/transactions/fetcher.rs#L1224)
    [***PartiallyFilterMessage::partially_filter_valid_entries*** fn emits ***trace*** event](crates/net/network/src/transactions/validation.rs#L74)
    [***EthMessageFilter::should_fetch*** fn emits ***trace*** events](crates/net/network/src/transactions/validation.rs#L162)
    [***EthMessageFilter::filter_valid_entries_68*** fn emits ***trace*** event](crates/net/network/src/transactions/validation.rs#L273)
    [***EthMessageFilter::filter_valid_entries_66*** fn emits ***trace*** event](crates/net/network/src/transactions/validation.rs#L321)
    [***TransactionsManager::on_get_pooled_transactions*** fn emits ***trace*** event](crates/net/network/src/transactions/mod.rs#L358)
    [***TransactionsManager::on_new_pending_transactions*** fn emits ***trace*** event](crates/net/network/src/transactions/mod.rs#L398)
    [***TransactionsManager::propagate_transactions*** fn emits ***trace*** events](crates/net/network/src/transactions/mod.rs#L432)
    [***TransactionsManager::propagate_full_transactions_to_peer*** fn emits ***trace*** event](crates/net/network/src/transactions/mod.rs#L519)
    [***TransactionsManager::propagate_hashes_to*** fn emits ***trace*** events](crates/net/network/src/transactions/mod.rs#L589)
    [***TransactionsManager::on_new_pooled_transaction_hashes*** fn emits ***trace*** events](crates/net/network/src/transactions/mod.rs#L651)
    [***TransactionsManager::import_transactions*** fn emits ***trace*** events](crates/net/network/src/transactions/mod.rs#L997)
    [***TransactionsManager::on_fetch_event*** fn emits ***trace*** events](crates/net/network/src/transactions/mod.rs#L1153)
    [***TransactionsManager::report_peer*** fn emits ***trace*** event](crates/net/network/src/transactions/mod.rs#L1185)
    [***TransactionsManager::report_already_seen*** fn emits ***trace*** event](crates/net/network/src/transactions/mod.rs#L1203)
    [***Discovery::poll*** fn emits ***trace*** event](crates/net/network/src/discovery.rs#L252)
    [***ConnectionListener::poll*** fn emits ***warn*** event](crates/net/network/src/listener.rs#L41)
    [***NetworkManager::on_invalid_message*** fn emits ***trace*** event](crates/net/network/src/manager.rs#L396)
    [***NetworkManager::delegate_eth_request*** fn emits ***debug*** event](crates/net/network/src/manager.rs#L419)
    [***NetworkManager::on_peer_message*** fn emits ***debug*** event](crates/net/network/src/manager.rs#L514)
    [***NetworkManager::on_handle_message*** fn emits ***warn*** event](crates/net/network/src/manager.rs#L554)
    [***NetworkManager::on_swarm_event*** fn emits ***trace, debug*** event](crates/net/network/src/manager.rs#L649)
    [***PeersManager::new*** fn emits ***warn*** event](crates/net/network/src/peers.rs#L93)
    [***PeersManager::backoff_peer_until*** fn emits ***trace*** event](crates/net/network/src/peers.rs#L400)
    [***PeersManager::on_connection_failure*** fn emits ***trace*** events](crates/net/network/src/peers.rs#L582)
    [***PeersManager::set_discovered_fork_id*** fn emits ***trace*** event](crates/net/network/src/peers.rs#L684)
    [***PeersManager::add_peer_kind*** fn emits ***trace*** event](crates/net/network/src/peers.rs#L714)
    [***PeersManager::remove_peer*** fn emits ***trace*** event](crates/net/network/src/peers.rs#L754)
    [***PeersManager::add_and_connect_kind*** fn emits ***trace*** event](crates/net/network/src/peers.rs#L793)
    [***PeersManager::fill_outbound_slots*** fn emits ***trace*** event](crates/net/network/src/peers.rs#L878)
    [***NetworkState::ban_ip_discovery*** fn emits ***trace*** event](crates/net/network/src/state.rs#L284)
    [***NetworkState::ban_discovery*** fn emits ***trace*** event](crates/net/network/src/state.rs#L290)
    [***NetworkState::poll*** fn emits ***debug*** event](crates/net/network/src/state.rs#L429)
    [***Swarm::on_session_event*** fn emits ***trace*** event](crates/net/network/src/swarm.rs#L115)
    [***Swarm::on_connection*** fn emits ***trace*** events](crates/net/network/src/swarm.rs#L184)
    [***Swarm::on_state_action*** fn emits ***trace*** events](crates/net/network/src/swarm.rs#L184)

24. **reth-network-types**
    [***PeersConfig::with_basic_nodes_from_file*** fn emits ***info*** event](crates/net/network-types/src/peers/config.rs#L279)
    [***Peer::apply_reputation*** fn emits ***trace*** event](crates/net/network-types/src/peers/mod.rs#L86)

25. **reth-network-p2p**
    [***FetchFullBlockFuture::take_block*** fn emits ***debug*** event](crates/net/p2p/src/full_block.rs#L147)
    [***FetchFullBlockFuture::on_block_response*** fn emits ***debug*** event](crates/net/p2p/src/full_block.rs#L171)
    [***FetchFullBlockFuture::poll*** fn emits ***debug*** events](crates/net/p2p/src/full_block.rs#L191)
    [***FetchFullBlockFuture::take_blocks*** fn emits ***debug*** events](crates/net/p2p/src/full_block.rs#L395)
    [***FetchFullBlockFuture::on_headers_response*** fn emits ***debug*** events](crates/net/p2p/src/full_block.rs#L452)
    [***FetchFullBlockRangeFuture::poll*** fn emits ***debug*** events](crates/net/p2p/src/full_block.rs#L514)

26. **reth-node-builder**
    [***BuilderContext::start_network_with*** fn emits ***tace, warn, info*** events](crates/node/builder/src/builder/mod.rs#L660)
    [***LaunchContext::load_toml_config*** fn emits ***info*** events](crates/node/builder/src/launch/common.rs#L126)
    [***LaunchContext::save_pruning_config_if_full_node*** fn emits ***info, warn*** events](crates/node/builder/src/launch/common.rs#L146)
    [***LaunchContext::configure_globals*** fn emits ***debug, error*** events](crates/node/builder/src/launch/common.rs#L173)
    [***LaunchContextWith::with_resolved_peers*** fn emits ***info*** event](crates/node/builder/src/launch/common.rs#L251)
    [***LaunchContextWith::create_provider_factory*** fn emits ***info*** event](crates/node/builder/src/launch/common.rs#L407)
    [***LaunchContextWith::start_prometheus_endpoint*** fn emits ***info*** event](crates/node/builder/src/launch/common.rs#L511)
    [***LaunchContextWith::with_metrics_task*** fn emits ***debug*** event](crates/node/builder/src/launch/common.rs#L552)
    [***LaunchContextWith::with_components*** fn emits ***debug*** events](crates/node/builder/src/launch/common.rs#L665)
    [***LaunchContextWith::check_pipeline_consistency*** fn emits ***debug*** event](crates/node/builder/src/launch/common.rs#L828)
    [***EngineNodeLauncher::launch_node*** fn emits ***info, debug, error*** events](crates/node/builder/src/launch/engine.rs#L83)
    [***ExExLauncher::launch*** fn emits ***info, debug*** events](crates/node/builder/src/launch/exex.rs#L43)
    [***DefaultNodeLauncher::launch_node*** fn emits ***info, debug*** events](crates/node/builder/src/launch/mod.rs#L108)
    [***RpcAddOns::launch_add_ons*** fn emits ***info, debug*** events](crates/node/builder/src/rpc.rs#L413)
    [***build_pipeline*** fn emits ***debug*** event](crates/node/builder/src/setup.rs#L72)

27. **reth-node-core**
    [***NetworkArgs::resolved_addr*** fn emits ***error*** event](crates/node/core/src/args/network.rs#L161)
    [***NodeConfig::lookup_or_fetch_tip*** fn emits ***info*** event](crates/node/core/src/node_config.rs#L328)
    [***NodeConfig::fetch_tip_from_network*** fn emits ***info, error*** events](crates/node/core/src/node_config.rs#L352)
    [***get_or_create_jwt_secret_from_path*** fn emits ***info, debug*** events](crates/node/core/src/utils.rs#L26)
    [***get_single_header*** fn emits ***info, debug*** events](crates/node/core/src/utils.rs#L26)

28. **reth-node-events**
    [***NodeState::handle_pipeline_event*** fn emits ***info*** events](crates/node/events/src/node.rs#L89)
    [***NodeState::handle_consensus_engine_event*** fn emits ***info*** events](crates/node/events/src/node.rs#L221)
    [***NodeState::handle_consensus_layer_health_event*** fn emits ***warn*** events](crates/node/events/src/node.rs#L284)
    [***NodeState::handle_pruner_event*** fn emits ***info*** events](crates/node/events/src/node.rs#L305)
    [***NodeState::handle_static_file_producer_event*** fn emits ***info*** events](crates/node/events/src/node.rs#316)
    [***EventHandler::poll*** fn emits ***info, warn*** events](crates/node/events/src/node.rs#446)
    [***Eta::update*** fn emits ***debug*** events](crates/node/events/src/node.rs#571)

29. **reth-node-metrics**
    [***Hooks::new*** fn emits ***error*** event](crates/node/metrics/src/hooks.rs#25)
    [***collect_memory_stats*** fn emits ***error*** events](crates/node/metrics/src/hooks.rs#49)
    [***collect_io_stats*** fn emits ***error*** events](crates/node/metrics/src/hooks.rs#100)
    [***MetricServer::start_endpoint*** fn emits ***debug, error*** events](crates/node/metrics/src/server.rs#77)

30. **op-reth**
    [***main*** fn emits ***warn*** event](crates/optimism/bin/src/main.rs#16)

31. **reth-optimism-cli**
    [***ImportReceiptsOpCommand::execute*** fn emits ***info, debug*** events](crates/optimism/cli/src/commands/import_receipts.rs#L51)
    [***import_receipts_from_file*** fn emits ***trace, info*** events](crates/optimism/cli/src/commands/import_receipts.rs#L84)
    [***import_receipts_from_reader*** fn emits ***warn, info*** events](crates/optimism/cli/src/commands/import_receipts.rs#L123)
    [***ImportOpCommand::execute*** fn emits ***info, debug, error*** events](crates/optimism/cli/src/commands/import_receipts.rs#L123)
    [***InitStateCommandOp::execute*** fn emits ***info*** events](crates/optimism/cli/src/commands/init_state.rs#L39)
    [***Cli::run*** fn emits ***info*** events](crates/optimism/cli/src/lib.rs#L132)

32. **reth-optimism-consensus**
    [***validate_block_post_execution*** fn emits ***debug*** events](crates/optimism/consensus/src/validation.rs#L11)

33. **reth-optimism-evm**
    [***OpExecutionStrategy::execute_transactions*** fn emits ***trace*** event](crates/optimism/evm/src/execute.rs#L152)
    [***ensure_create2_deployer*** fn emits ***trace*** event](crates/optimism/evm/src/execute.rs#L260)

34. **reth-optimism-node**
    [***OpPoolBuilder::build_pool*** fn emits ***info, debug*** events](crates/optimism/node/src/node.rs#L255)
    [***OpNetworkBuilder::build_network*** fn emits ***info*** event](crates/optimism/node/src/node.rs#L452)

35. **reth-optimism-payload-builder**
    [***OpBuilder::build*** fn emits ***warn, debug***](crates/optimism/payload/src/builder.rs#L238)
    [***OpPayloadBuilderCtx::ensure_create2_deployer*** fn emits ***warn*** events](crates/optimism/payload/src/builder.rs#L580)
    [***OpPayloadBuilderCtx::apply_pre_beacon_root_contract_call*** fn emits ***warn*** events](crates/optimism/payload/src/builder.rs#L602)
    [***OpPayloadBuilderCtx::execute_sequencer_transactions*** fn emits ***trace*** events](crates/optimism/payload/src/builder.rs#L630)
    [***OpPayloadBuilderCtx::execute_best_transactions*** fn emits ***trace*** events](crates/optimism/payload/src/builder.rs#L729)

36. **reth-optimism-rpc**
    [***OpEthApi::send_raw_transaction*** fn emits ***debug*** events](crates/optimism/rpc/src/eth/transaction.rs#L32)
    [***SequencerClient::forward_raw_transaction*** fn emits ***warn*** events](crates/optimism/rpc/src/sequencer.rs#L54)

37. **reth-payload-builder**
    [***PayloadBuilderService::resolve*** fn emits ***trace*** events](crates/payload/builder/src/service.rs#L294)
    [***PayloadBuilderService::payload_attributes*** fn emits ***trace*** event](crates/payload/builder/src/service.rs#L348)
    [***PayloadBuilderService::poll*** fn emits ***trace, info, warn, debug*** events](crates/payload/builder/src/service.rs#L366)

38. **reth-payload-primitives**
    [***BuiltPayloadStream::poll_next*** fn emits ***debug*** event](crates/payload/primitives/src/events.rs#L66)
    [***PayloadAttributeStream::poll_next*** fn emits ***debug*** event](crates/payload/primitives/src/events.rs#L96)

39. **reth-prune**
    [Instrumented fn ***AccountHistory::prune*** at ***trace*** level. It also emits ***trace*** events](crates/prune/prune/src/segments/user/account_history.rs#L51)
    [Instrumented fn ***ReceiptsByLogs::prune*** at ***trace*** level. It also emits ***trace*** events](crates/prune/prune/src/segments/user/account_history.rs#L51)
    [Instrumented fn ***Receipts::prune*** at ***trace*** level](crates/prune/prune/src/segments/user/receipts.rs#L41)
    [Instrumented fn ***SenderRecovery::prune*** at ***trace*** level. It also emits ***trace*** events](crates/prune/prune/src/segments/user/sender_recovery.rs#L41)
    [Instrumented fn ***TransactionLookup::prune*** at ***trace*** level. It also emits ***trace*** events](crates/prune/prune/src/segments/user/transaction_lookup.rs#L42)
    [***Headers::prune*** fn emits ***trace*** event](crates/prune/prune/src/segments/static_file/headers.rs#L52)
    [***Transactions::prune*** fn emits ***trace*** events](crates/prune/prune/src/segments/static_file/transactions.rs#L43)
    [***prune*** fn emits ***trace*** events](crates/prune/prune/src/segments/receipts.rs#L19)
    [***PruneInput::get_next_tx_num_range*** fn emits ***error*** event](crates/prune/prune/src/segments/mod.rs#L77)
    [***DbTxPruneExt::prune_table_with_iterator*** fn emits ***debug*** event](crates/prune/prune/src/db_ext.rs#L16)
    [***Pruner::run_with_provider*** fn emits ***debug*** events](crates/prune/prune/src/pruner.rs#L115)
    [***Pruner::prune_segments*** fn emits ***debug*** events](crates/prune/prune/src/pruner.rs#L177)
    [***Pruner::is_pruning_needed*** fn emits ***debug*** event](crates/prune/prune/src/pruner.rs#L268)
    [***Pruner::adjust_tip_block_number_to_finished_exex_height*** fn emits ***debug*** event](crates/prune/prune/src/pruner.rs#L300)

40. **reth-ipc**
    [Instrumented fn ***process_batch_request*** at ***trace*** level](crates/rpc/ipc/src/server/ipc.rs#L35)
    [Instrumented fn ***execute_call_with_tracing*** at ***trace*** level](crates/rpc/ipc/src/server/ipc.rs#L106)
    [Instrumented fn ***execute_notification*** at ***trace*** level](crates/rpc/ipc/src/server/ipc.rs#L117)
    [Instrumented fn ***EngineEthApi::call*** at ***info*** level](crates/rpc/rpc/src/engine.rs#L69)
    [***Sender::send_ping*** fn emits ***debug*** event](crates/rpc/ipc/src/client/mod.rs#L33)
    [***IpcConnDriver::poll*** fn emits ***warn, debug*** events](crates/rpc/ipc/src/server/connection.rs#L91)
    [***RpcService::call*** fn emits ***warn*** events](crates/rpc/ipc/src/server/rpc_service.rs#L54)
    [***EthFilter::clear_stale_filters*** fn emits ***trace*** events](crates/rpc/rpc/src/eth/filter.rs#L131)
    [***EthFilter::new_filter*** fn emits ***trace*** event](crates/rpc/rpc/src/eth/filter.rs#L252)
    [***EthFilter::new_block_filter*** fn emits ***trace*** event](crates/rpc/rpc/src/eth/filter.rs#L260)
    [***EthFilter::new_pending_transaction_filter*** fn emits ***trace*** event](crates/rpc/rpc/src/eth/filter.rs#L266)
    [***EthFilter::filter_changes*** fn emits ***trace*** event](crates/rpc/rpc/src/eth/filter.rs#L295)
    [***EthFilter::filter_logs*** fn emits ***trace*** event](crates/rpc/rpc/src/eth/filter.rs#L308)
    [***EthFilter::uninstall_filter*** fn emits ***trace*** events](crates/rpc/rpc/src/eth/filter.rs#L314)
    [***EthFilter::logs*** fn emits ***trace*** event](crates/rpc/rpc/src/eth/filter.rs#L328)
    [***EthFilterInner::get_logs_in_block_range*** fn emits ***trace*** event](crates/rpc/rpc/src/eth/filter.rs#L454)
    [***handle_accepted*** fn emits ***error*** event](crates/rpc/rpc/src/eth/pubsub.rs#L110)
    [***EthSimBundle::sim_bundle*** fn emits ***info*** event](crates/rpc/rpc/src/eth/sim_bundle.rs#L448)
    [***TxPoolApi::txpool_status*** fn emits ***trace*** event](crates/rpc/rpc/src/txpool.rs#L83)
    [***TxPoolApi::txpool_inspect*** fn emits ***trace*** event](crates/rpc/rpc/src/txpool.rs#L95)
    [***TxPoolApi::txpool_content_from*** fn emits ***trace*** event](crates/rpc/rpc/src/txpool.rs#L135)
    [***TxPoolApi::txpool_content*** fn emits ***trace*** event](crates/rpc/rpc/src/txpool.rs#L149)

41. **reth-rpc-builder**
    [***TxPoolApi::txpool_content*** fn emits ***trace*** event](crates/rpc/rpc/src/txpool.rs#L149)
    [***RpcServerArgs::rpc_server_config*** fn emits ***warn*** event](crates/rpc/rpc-builder/src/config.rs#L176)
    [***RpcServerArgs::auth_jwt_secret*** fn emits ***debug*** event](crates/rpc/rpc-builder/src/config.rs#L220)

42. **reth-rpc-engine-api**
    [***EngineApi::get_payload_v1*** fn emits ***warn*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L359)
    [***EngineApi::get_payload_v2*** fn emits ***warn*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L383)
    [***EngineApi::get_payload_v3*** fn emits ***warn*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L418)
    [***EngineApi::get_payload_v4*** fn emits ***warn*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L418)
    [***EngineApi::new_payload_v1*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L747)
    [***EngineApi::new_payload_v2*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L754)
    [***EngineApi::new_payload_v3*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L761)
    [***EngineApi::new_payload_v4*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L773)
    [***EngineApi::fork_choice_updated_v1*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L795)
    [***EngineApi::fork_choice_updated_v2*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L810)
    [***EngineApi::fork_choice_updated_v3*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L826)
    [***EngineApi::get_payload_v1*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L850)
    [***EngineApi::get_payload_v2*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L870)
    [***EngineApi::get_payload_v3*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L890)
    [***EngineApi::get_payload_v4*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L910)
    [***EngineApi::get_payload_bodies_by_hash_v1*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L923)
    [***EngineApi::get_payload_bodies_by_range_v1*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L950)
    [***EngineApi::exchange_transition_configuration*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L964)
    [***EngineApi::get_client_version_v1*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L978)
    [***EngineApi::get_blobs_v1*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L994)

43. **reth-rpc-eth-types**
    [***RpcInvalidTransactionError::from*** fn emits ***error*** event](crates/rpc/rpc-eth-types/src/error/mod.rs#L494)
    [***fee_history_cache_new_blocks_task*** fn emits ***trace*** event](crates/rpc/rpc-eth-types/src/fee_history.rs#L208)
    [***GasPriceOracle::new*** fn emits ***warn*** event](crates/rpc/rpc-eth-types/src/gas_oracle.rs#L90)
    [***JwtAuthValidator::validate*** fn emits ***error*** events](crates/rpc/rpc-layer/src/jwt_validator.rs#L24)

44. **reth-stages-api**
    [***MetricsListener::handle_event*** fn emits ***trace*** event](crates/stages/api/src/metrics/listener.rs#L48)
    [***Pipeline::set_tip*** fn emits ***warn*** event](crates/stages/api/src/pipeline/mod.rs#L101)
    [***Pipeline::run_as_fut*** fn emits ***trace*** event](crates/stages/api/src/pipeline/mod.rs#L133)
    [***Pipeline::run*** fn emits ***trace*** event](crates/stages/api/src/pipeline/mod.rs#L164)
    [***Pipeline::run_loop*** fn emits ***trace*** events](crates/stages/api/src/pipeline/mod.rs#L205)
    [***Pipeline::unwind*** fn emits ***info, debug*** events](crates/stages/api/src/pipeline/mod.rs#L278)
    [***Pipeline::execute_stage_to_completion*** fn emits ***info, debug*** events](crates/stages/api/src/pipeline/mod.rs#L382)
    [***on_stage_error*** fn emits ***warn, error*** events](crates/stages/api/src/pipeline/mod.rs#L382)

45. **reth-stages**
    [***BodyStage::execute*** fn emits ***trace, debug*** events](crates/stages/stages/src/stages/bodies.rs#L113)
    [***ExecutionStage::execute*** fn emits ***trace, info, debug*** events](crates/stages/stages/src/stages/execution.rs#L197)
    [***ExecutionStage::unwind*** fn emits ***debug*** events](crates/stages/stages/src/stages/execution.rs#L397)
    [***calculate_gas_used_from_headers*** fn emits ***debug*** events](crates/stages/stages/src/stages/execution.rs#L553)
    [***AccountHashingStage::execute*** fn emits ***info*** event](crates/stages/stages/src/stages/hashing_account.rs#L138)
    [***collect*** fn emits ***info*** event](crates/stages/stages/src/stages/hashing_account.rs#L249)
    [***StorageHashingStage::execute*** fn emits ***info*** event](crates/stages/stages/src/stages/hashing_storage.rs#L75)
    [***collect*** fn emits ***info*** event](crates/stages/stages/src/stages/hashing_storage.rs#L187)
    [***HeaderStage::write_headers*** fn emits ***info*** events](crates/stages/stages/src/stages/headers.rs#L93)
    [***HeaderStage::poll_execute_ready*** fn emits ***info, debug, error*** events](crates/stages/stages/src/stages/headers.rs#L205)
    [***IndexAccountHistoryStage::execute*** fn emits ***info*** events](crates/stages/stages/src/stages/index_account_history.rs#L56)
    [***IndexStorageHistoryStage::execute*** fn emits ***info*** events](crates/stages/stages/src/stages/index_storage_history.rs#L58)
    [***MerkleStage::save_execution_checkpoint*** fn emits ***debug*** event](crates/stages/stages/src/stages/merkle.rs#L115)
    [***MerkleStage::unwind*** fn emits ***info*** event](crates/stages/stages/src/stages/merkle.rs#L289)
    [***validate_state_root*** fn emits ***error*** event](crates/stages/stages/src/stages/merkle.rs#L346)
    [***PruneStage::execute*** fn emits ***info*** events](crates/stages/stages/src/stages/prune.rs#L50)
    [***SenderRecoveryStage::execute*** fn emits ***info*** events](crates/stages/stages/src/stages/sender_recovery.rs#L75)
    [***recover_range*** fn emits ***debug*** events](crates/stages/stages/src/stages/sender_recovery.rs#L141)
    [***TransactionLookupStage::execute*** fn emits ***trace, info*** events](crates/stages/stages/src/stages/tx_lookup.rs#L72)
    [***TransactionLookupStage::unwind*** fn emits ***trace, info*** events](crates/stages/stages/src/stages/tx_lookup.rs#L187)
    [***collect_history_indices*** fn emits ***info*** events](crates/stages/stages/src/stages/utils.rs#L38)
    [***load_history_indices*** fn emits ***info*** events](crates/stages/stages/src/stages/utils.rs#L105)    
