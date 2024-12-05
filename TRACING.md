# Tracing targets

This document highlights instrumented functions and tracing events emitted by them.

### Crates

#### reth-blockchain-tree

Targets:

1. **blockchain_tree**
    [Instrumented ***try_insert_validated_block*** fn with ***trace*** level](crates/blockchain-tree/src/blockchain_tree.rs#L319)
    [Instrumented ***try_append_canonical_chain*** fn with ***trace*** level. ***debug*** event is emitted](crates/blockchain-tree/src/blockchain_tree.rs#L384)
    [Instrumented ***try_insert_block_into_side_chain*** fn with ***trace*** level. ***debug*** event is emitted](crates/blockchain-tree/src/blockchain_tree.rs#L448)
    [Instrumented ***make_canonical*** fn with ***trace*** level. Emits ***info, trace, debug, error*** event](crates/blockchain-tree/src/blockchain_tree.rs#L1020)
    [***post_state_data*** fn emits events at ***trace, debug*** levels](crates/blockchain-tree/src/blockchain_tree.rs#L267)
    [***find_all_dependent_chains*** fn emits events at ***debug*** level](crates/blockchain-tree/src/blockchain_tree.rs#L587)
    [***insert_unwound_chain*** fn emits events at ***debug*** level](crates/blockchain-tree/src/blockchain_tree.rs#L624)
    [***validate_block*** fn emits events at ***error*** level](crates/blockchain-tree/src/blockchain_tree.rs#L697)
    [***is_block_inside_sidechain*** fn emits event at ***debug*** level](crates/blockchain-tree/src/blockchain_tree.rs#L728)
    [***try_connect_buffered_blocks*** fn emits events at ***trace, debug*** levels](crates/blockchain-tree/src/blockchain_tree.rs#L921)
    [***remove_and_split_chain*** fn emits events at ***trace*** levels](crates/blockchain-tree/src/blockchain_tree.rs#L943)    
    [***commit_canonical_to_database*** fn emits events at ***debug*** levels](crates/blockchain-tree/src/blockchain_tree.rs#L1212)
    [***revert_canonical_from_database*** fn emits events at ***trace, info*** levels](crates/blockchain-tree/src/blockchain_tree.rs#L1302)

2. **reth-chain-state**
    [***CanonStateNotificationStream::poll_next*** fn emits ***debug*** event](crates/chain-state/src/notifications.rs#L64)

3. **reth-cli-commands**
    [***Command::execute*** fn emits ***warn*** event](crates/cli/commands/src/db/checksum.rs#L42)
    [***ChecksumViewer::view*** fn emits ***info*** events](crates/cli/commands/src/db/checksum.rs#L73)
    [***Command::execute*** fn emits ***warn*** event](crates/cli/commands/src/db/diff.rs#L55)
    [***find_diffs*** fn emits ***info*** events](crates/cli/commands/src/db/diff.rs#L91)
    [***Command::execute*** fn emits ***error*** event](crates/cli/commands/src/db/get.rs#L59)
    [***ListTableViewer::view*** fn emits ***error*** event](crates/cli/commands/src/db/list.rs#L97)
    [***DBList::run*** fn emits ***error*** event](crates/cli/commands/src/db/tui.rs#L212)
    [***setup_without_evm*** fn emits ***info*** events](crates/cli/commands/src/init_state/without_evm.rs#L29)
    [***InitStateCommand::execute*** fn emits ***info*** events](crates/cli/commands/src/init_state/mod.rs#L70)
    [***Command::execute*** fn emits ***info*** events](crates/cli/commands/src/init_state/mod.rs#L70)
    [***dry_run*** fn emits ***info*** events](crates/cli/commands/src/stage/dump/execution.rs#L180)
    [***dry_run*** fn emits ***info*** events](crates/cli/commands/src/stage/dump/hashing_account.rs#L78)
    [***dry_run*** fn emits ***info*** events](crates/cli/commands/src/stage/dump/hashing_storage.rs#L73)
    [***dry_run*** fn emits ***info*** events](crates/cli/commands/src/stage/dump/merkle.rs#L167)
    [***setup*** fn emits ***info*** event](crates/cli/commands/src/stage/dump/mod.rs#L118)
    [***Command::execute*** fn emits ***info*** events](crates/cli/commands/src/stage/unwind.rs#L53)
    [***Command::execute*** fn emits ***info*** events](crates/cli/commands/src/stage/run.rs#L107)
    [***EnvironmentArgs::init*** fn emits ***info, debug*** events](crates/cli/commands/src/common.rs#L60)
    [***EnvironmentArgs::init*** fn emits ***info, warn*** events](crates/cli/commands/src/common.rs#L60)
    [***ImportCommand::execute*** fn emits ***info, debug, error*** events](crates/cli/commands/src/import.rs#L60)
    [***InitCommand::execute*** fn emits ***info*** events](crates/cli/commands/src/import.rs#L60)
    [***NodeCommand::execute*** fn emits ***info*** events](crates/cli/commands/src/node.rs#L140)
    [***PruneCommand::execute*** fn emits ***info*** events](crates/cli/commands/src/prune.rs#L19)
    [***CliRunner::run_command_until_exit*** fn emits ***debug, error*** events](crates/cli/runner/src/lib.rs#L32)
    [***CliRunner::run_blocking_until_ctrl_c*** fn emits ***trace*** events](crates/cli/runner/src/lib.rs#L172)

5. **reth-beacon-consensus**
    [Instrumented ***BeaconConsensusEngine::on_new_payload*** fn with ***trace*** level. Fn emits error event.](crates/consensus/beacon/src/engine/mod.rs#L1091)
    [Instrumented ***BeaconConsensusEngine::try_buffer_payload*** fn with ***trace*** level.](crates/consensus/beacon/src/engine/mod.rs#L1226)
    [Instrumented ***BeaconConsensusEngine::try_insert_new_payload*** fn with ***trace*** level.](crates/consensus/beacon/src/engine/mod.rs#L1225)
    [***InvalidHeaderCache::insert_with_invalid_ancestor*** fn emits ***warn*** event](crates/consensus/beacon/src/engine/invalid_headers.rs#L56)
    [***InvalidHeaderCache::insert*** fn emits ***warn*** event](crates/consensus/beacon/src/engine/invalid_headers.rs#L68)
    [***EngineSyncController::download_block_range*** fn emits ***trace*** event](crates/consensus/beacon/src/engine/sync.rs#L147)
    [***EngineSyncController::download_full_block*** fn emits ***trace*** event](crates/consensus/beacon/src/engine/sync.rs#L177)
    [***EngineSyncController::set_pipeline_sync_target*** fn emits ***trace*** event](crates/consensus/beacon/src/engine/sync.rs#L206)
    [***EngineSyncController::has_reached_max_block*** fn emits ***trace*** event](crates/consensus/beacon/src/engine/sync.rs#L221)
    [***EngineSyncController::poll*** fn emits ***trace*** events](crates/consensus/beacon/src/engine/sync.rs#L289)
    [***BeaconConsensusEngine::pre_validate_forkchoice_update*** fn emits ***trace*** events](crates/consensus/beacon/src/engine/mod.rs#L366)
    [***BeaconConsensusEngine::on_forkchoice_updated_make_canonical_result*** fn emits ***trace, debug, error*** events](crates/consensus/beacon/src/engine/mod.rs#L396)
    [***BeaconConsensusEngine::on_head_already_canonical*** fn emits ***debug*** events](crates/consensus/beacon/src/engine/mod.rs#L464)
    [***BeaconConsensusEngine::on_forkchoice_updated*** fn emits ***trace, warn*** events](crates/consensus/beacon/src/engine/mod.rs#L509)
    [***BeaconConsensusEngine::check_pipeline_consistency*** fn emits ***debug*** events](crates/consensus/beacon/src/engine/mod.rs#L598)
    [***BeaconConsensusEngine::can_pipeline_sync_to_finalized*** fn emits ***warn, debug*** events](crates/consensus/beacon/src/engine/mod.rs#L653)
    [***BeaconConsensusEngine::on_failed_canonical_forkchoice_update*** fn emits ***trace, warn, debug*** events](crates/consensus/beacon/src/engine/mod.rs#L989)
    [***BeaconConsensusEngine::try_make_sync_target_canonical*** fn emits ***debug*** events](crates/consensus/beacon/src/engine/mod.rs#L1341)
    [***BeaconConsensusEngine::on_sync_event*** fn emits ***trace, error*** events](crates/consensus/beacon/src/engine/mod.rs#L1404)
    [***BeaconConsensusEngine::on_pipeline_outcome*** fn emits ***warn, error*** events](crates/consensus/beacon/src/engine/mod.rs#L1454)
    [***BeaconConsensusEngine::set_canonical_head*** fn emits ***error*** event](crates/consensus/beacon/src/engine/mod.rs#L1554)
    [***BeaconConsensusEngine::on_hook_result*** fn emits ***error*** events](crates/consensus/beacon/src/engine/mod.rs#L1565)
    [***BeaconConsensusEngine::on_blockchain_tree_action*** fn emits ***trace, warn, debug, error*** events](crates/consensus/beacon/src/engine/mod.rs#L1607)
    [***EtherscanBlockProvider::subscribe_blocks*** fn emits ***warn*** events](crates/consensus/debug-client/src/providers/etherscan.rs#L54)
    [***DebugConsensusClient::run*** fn emits ***warn*** events](crates/consensus/debug-client/src/client.rs#L74)

6. **reth-e2e-test-utils**
    [***InvalidBlockWitnessHook::on_invalid_block*** fn emits ***info*** events](crates/engine/invalid-block-hooks/src/witness.rs#L58)
    [***InvalidBlockWitnessHook::on_invalid_block*** fn emits ***info*** event](crates/engine/invalid-block-hooks/src/witness.rs#L309)

7. **reth-engine-local**
    [***LocalMiner::run*** fn emits ***error*** events](crates/engine/local/src/miner.rs#L127)

8. **reth-engine-local**
    [***PersistenceState::schedule_removal*** fn emits ***debug*** event](crates/engine/tree/src/tree/persistence_state.rs#L36)
    [***PersistenceState::schedule_removal*** fn emits ***trace*** event](crates/engine/tree/src/tree/persistence_state.rs#L42)
    [***PipelineSync::set_pipeline_sync_target*** fn emits ***trace*** event](crates/engine/tree/src/backfill.rs#L120)

9. **reth-engine-tree**
    [Instrumented ***ChainOrchestrator::poll_next_event*** fn at ***debug*** level. Fn emits ***debug, error*** events.](crates/engine/tree/src/chain.rs#L75)
    [***BasicBlockDownloader::download_block_range*** fn emits ***trace*** event](crates/engine/tree/src/download.rs#L117)
    [***BasicBlockDownloader::download_full_block*** fn emits ***trace*** event](crates/engine/tree/src/download.rs#L141)
    [***BasicBlockDownloader::poll*** fn emits ***trace*** event](crates/engine/tree/src/download.rs#L200)
    [***PersistenceService::prune_before*** fn emits ***debug*** event](crates/engine/tree/src/persistence.rs#L54)
    [***PersistenceService::on_remove_blocks_above*** fn emits ***debug*** event](crates/engine/tree/src/persistence.rs#L127)
    [***PersistenceService::on_save_blocks*** fn emits ***debug*** event](crates/engine/tree/src/persistence.rs#L145)
    [***PersistenceHandle::spawn_service*** fn emits ***error*** event](crates/engine/tree/src/persistence.rs#L216)

10. **reth-engine-util**
    [***EngineMessageStore::engine_messages_iter*** fn emits ***warn, debug*** event](crates/engine/util/src/engine_store.rs#L100)
    [***EngineStoreStream::poll_next*** fn emits ***error*** event](crates/engine/util/src/reorg.rs#L118)
    [***EngineReorg::poll_next*** fn emits ***debug, error*** event](crates/engine/util/src/engine_store.rs#L146)
    [***create_reorg_head*** fn emits ***trace, debug*** events](crates/engine/util/src/reorg.rs#L250)
    [***EngineSkipFcu::poll_next*** fn emits ***warn*** event](crates/engine/util/src/skip_fcu.rs#L41)
    [***EngineSkipNewPayload::poll_next*** fn emits ***warn*** event](crates/engine/util/src/skip_new_payload.rs#L37)

11. **reth-ethereum-consensus**
    [***validate_block_post_execution*** fn emits ***debug*** event](crates/ethereum/consensus/src/validation.rs#L11)

12. **reth-node-ethereum**
    [***EthereumPoolBuilder::build_pool*** fn emits ***info, debug*** events](crates/ethereum/node/src/node.rs#L172)
    [***EthereumNetworkBuilder::build_network*** fn emits ***info*** event](crates/ethereum/node/src/node.rs#L320)

13. **reth-ethereum-payload-builder**
    [***default_ethereum_payload*** fn emits ***trace, warn, debug*** events](crates/ethereum/payload/src/lib.rs#L150)

14. **reth-exex**
    [Instrumented ***StreamBackfillJob::read_notification*** fn emits ***debug*** event](crates/exex/exex/src/wal/storage.rs#L130)
    [Instrumented ***StreamBackfillJob::write_notification*** fn emits ***debug*** event](crates/exex/exex/src/wal/storage.rs#L159)
    [Instrumented ***Storage::remove_notification*** fn emits ***debug*** events](crates/exex/exex/src/wal/storage.rs#L56)
    [***BackfillJob::execute_range*** fn emits ***trace, debug*** events](crates/exex/exex/src/backfill/job.rs#L70)
    [***SingleBlockBackfillJob::execute_block*** fn emits ***trace*** event](crates/exex/exex/src/backfill/job.rs#L194)
    [***StreamBackfillJob::poll_next*** fn emits ***debug*** event](crates/exex/exex/src/backfill/stream.rs#L114)
    [***ExExNotificationsWithHead::check_canonical*** fn emits ***debug*** events](crates/exex/exex/src/notifications.rs#314)
    [***ExExNotificationsWithHead::check_backfill*** fn emits ***debug*** events](crates/exex/exex/src/notifications.rs#L357)
    [***ExExNotificationsWithHead::poll_next*** fn emits ***debug*** events](crates/exex/exex/src/notifications.rs#L357)

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
    [***BodiesDownloader::set_download_range*** fn emits ***trace, info, error*** events](crates/net/downloaders/src/bodies/bodies.rs#L314)
    [***BodiesDownloader::poll_next*** fn emits ***debug, error*** events](crates/net/downloaders/src/bodies/bodies.rs#L358)
    [***BodiesDownloader::on_error*** fn emits ***debug*** events](crates/net/downloaders/src/bodies/request.rs#L90)
    [***BodiesDownloader::submit_request*** fn emits ***trace*** event](crates/net/downloaders/src/bodies/request.rs#L110)
    [***BodiesDownloader::on_block_response*** fn emits ***trace*** event](crates/net/downloaders/src/bodies/request.rs#L119)
    [***SpawnedDownloader::poll*** fn emits ***trace*** event](crates/net/downloaders/src/bodies/request.rs#L119)
    [***SpawnedDownloader::poll*** fn emits ***trace*** event](crates/net/downloaders/src/bodies/task.rs#L121)
    [***ReverseHeadersDownloader::process_next_headers*** fn emits ***trace, error*** event](crates/net/downloaders/src/headers/reverse_headers.rs#L246)
    [***ReverseHeadersDownloader::on_sync_target_outcome*** fn emits ***trace*** event](crates/net/downloaders/src/headers/reverse_headers.rs#L355)
    [***ReverseHeadersDownloader::on_headers_outcome*** fn emits ***trace*** events](crates/net/downloaders/src/headers/reverse_headers.rs#L433)
    [***ReverseHeadersDownloader::penalize_peer*** fn emits ***trace*** events](crates/net/downloaders/src/headers/reverse_headers.rs#L519)
    [***ReverseHeadersDownloader::submit_request*** fn emits ***trace*** events](crates/net/downloaders/src/headers/reverse_headers.rs#L575)
    [***ReverseHeadersDownloader::update_sync_target*** fn emits ***trace*** events](crates/net/downloaders/src/headers/reverse_headers.rs#L681)
    [***ReverseHeadersDownloader::poll_next*** fn emits ***trace*** events](crates/net/downloaders/src/headers/reverse_headers.rs#L753)
    [***FileClient::from_reader*** fn emits ***trace*** events](crates/net/downloaders/src/file_client.rs#L189)
    [***FileClient::get_headers_with_priority*** fn emits ***trace, warn*** events](crates/net/downloaders/src/file_client.rs#L269)
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
    [***UnauthedEthStream::handshake_without_timeout*** fn emits ***trace, debug*** events](crates/net/eth-wire/src/ethstream.rs#L82)
    [***EthStream::poll_next*** fn emits ***debug*** event](crates/net/eth-wire/src/ethstream.rs#L277)
    [***UnauthedP2PStream::handshake*** fn emits ***trace, debug*** events](crates/net/eth-wire/src/p2pstream.rs#L89)
    [***UnauthedP2PStream::send_disconnect*** fn emits ***trace*** event](crates/net/eth-wire/src/p2pstream.rs#L178)
    [***P2PStream::start_disconnect*** fn emits ***debug*** event](crates/net/eth-wire/src/p2pstream.rs#L330)
    [***P2PStream::poll_next*** fn emits ***trace, debug*** events](crates/net/eth-wire/src/p2pstream.rs#L388)
    [***P2PStream::start_send*** fn emits ***debug*** event](crates/net/eth-wire/src/p2pstream.rs#L571)

22. **reth-net-nat**
    [***external_addr_with*** fn emits ***debug*** event](crates/net/nat/src/lib.rs#L198)
    [***external_addr_with*** fn emits ***debug*** event](crates/net/nat/src/lib.rs#L214)

23. **reth-network**
    [Instrumented fn ***start_pending_outbound_session***](crates/net/network/src/session/mod.rs#L856)
    [***ActiveSession::on_internal_peer_message*** fn emits ***debug*** event](crates/net/network/src/session/active.rs#L257)
    [***ActiveSession::handle_outgoing_response*** fn emits ***debug*** event](crates/net/network/src/session/active.rs#L296)
    [***ActiveSession::try_emit_broadcast*** fn emits ***trace*** event](crates/net/network/src/session/active.rs#L311)
    [***ActiveSession::try_emit_request*** fn emits ***trace*** event](crates/net/network/src/session/active.rs#L337)
    [***ActiveSession::emit_disconnect*** fn emits ***trace*** event](crates/net/network/src/session/active.rs#L369)
    [***ActiveSession::try_disconnect*** fn emits ***debug*** event](crates/net/network/src/session/active.rs#L410)
    [***ActiveSession::check_timed_out_requests*** fn emits ***debug*** event](crates/net/network/src/session/active.rs#L432)
    [***ActiveSession::poll*** fn emits ***trace, debug*** events](crates/net/network/src/session/active.rs#L480)
    [***SessionManager::on_incoming*** fn emits ***trace*** event](crates/net/network/src/session/mod.rs#L229)
    [***SessionManager::send_message*** fn emits ***debug*** event](crates/net/network/src/session/mod.rs#L350)
    [***SessionManager::try_disconnect_incoming_connection*** fn emits ***trace*** event](crates/net/network/src/session/mod.rs#L384)
    [***SessionManager::poll*** fn emits ***trace*** events](crates/net/network/src/session/mod.rs#L413)
    [***pending_session_with_timeout*** fn emits ***debug*** event](crates/net/network/src/session/mod.rs#L799)
    [***TransactionFetcher::buffer_hashes*** fn emits ***trace*** event](crates/net/network/src/transactions/fetcher.rs#L386)
    [***TransactionFetcher::on_fetch_pending_hashes*** fn emits ***trace*** events](crates/net/network/src/transactions/fetcher.rs#L430)
    [***TransactionFetcher::request_transactions_from_peer*** fn emits ***trace, debug*** events](crates/net/network/src/transactions/fetcher.rs#L632)
    [***TransactionFetcher::search_breadth_budget_find_idle_fallback_peer*** fn emits ***trace*** event](crates/net/network/src/transactions/fetcher.rs#L815)
    [***TransactionFetcher::search_breadth_budget_find_intersection_pending_hashes_and_hashes_seen_by_peer*** fn emits ***trace*** event](crates/net/network/src/transactions/fetcher.rs#L854)
    [***TransactionFetcher::on_resolved_get_pooled_transactions_request_fut*** fn emits ***trace*** events](crates/net/network/src/transactions/fetcher.rs#L903)
    [***UnverifiedPooledTransactions::verify*** fn emits ***trace*** events](crates/net/network/src/transactions/fetcher.rs#L1220)
    [***PartiallyFilterMessage::partially_filter_valid_entries*** fn emits ***trace*** event](crates/net/network/src/transactions/validation.rs#L73)
    [***EthMessageFilter::should_fetch*** fn emits ***trace*** events](crates/net/network/src/transactions/validation.rs#L161)
    [***EthMessageFilter::filter_valid_entries_68*** fn emits ***trace*** event](crates/net/network/src/transactions/validation.rs#L272)
    [***EthMessageFilter::filter_valid_entries_66*** fn emits ***trace*** event](crates/net/network/src/transactions/validation.rs#L320)
    [***TransactionsManager::on_get_pooled_transactions*** fn emits ***trace*** event](crates/net/network/src/transactions/mod.rs#L979)
    [***TransactionsManager::on_new_pending_transactions*** fn emits ***trace*** event](crates/net/network/src/transactions/mod.rs#L718)
    [***TransactionsManager::propagate_transactions*** fn emits ***trace*** events](crates/net/network/src/transactions/mod.rs#L876)
    [***TransactionsManager::propagate_full_transactions_to_peer*** fn emits ***trace*** event](crates/net/network/src/transactions/mod.rs#L735)
    [***TransactionsManager::propagate_hashes_to*** fn emits ***trace*** events](crates/net/network/src/transactions/mod.rs#L805)
    [***TransactionsManager::on_new_pooled_transaction_hashes*** fn emits ***trace*** events](crates/net/network/src/transactions/mod.rs#L480)
    [***TransactionsManager::import_transactions*** fn emits ***trace*** events](crates/net/network/src/transactions/mod.rs#L1163)
    [***TransactionsManager::on_fetch_event*** fn emits ***trace*** events](crates/net/network/src/transactions/mod.rs#L1306)
    [***TransactionsManager::report_peer*** fn emits ***trace*** event](crates/net/network/src/transactions/mod.rs#L352)
    [***TransactionsManager::report_already_seen*** fn emits ***trace*** event](crates/net/network/src/transactions/mod.rs#L357)
    [***Discovery::poll*** fn emits ***trace*** event](crates/net/network/src/discovery.rs#L250)
    [***ConnectionListener::poll*** fn emits ***warn*** event](crates/net/network/src/listener.rs#L40)
    [***NetworkManager::on_invalid_message*** fn emits ***trace*** event](crates/net/network/src/manager.rs#L402)
    [***NetworkManager::delegate_eth_request*** fn emits ***debug*** event](crates/net/network/src/manager.rs#L425)
    [***NetworkManager::on_peer_message*** fn emits ***debug*** event](crates/net/network/src/manager.rs#L520)
    [***NetworkManager::on_handle_message*** fn emits ***warn*** event](crates/net/network/src/manager.rs#L560)
    [***NetworkManager::on_swarm_event*** fn emits ***trace, debug*** event](crates/net/network/src/manager.rs#L658)
    [***PeersManager::new*** fn emits ***warn*** event](crates/net/network/src/peers.rs#L91)
    [***PeersManager::backoff_peer_until*** fn emits ***trace*** event](crates/net/network/src/peers.rs#L398)
    [***PeersManager::on_connection_failure*** fn emits ***trace*** events](crates/net/network/src/peers.rs#L580)
    [***PeersManager::set_discovered_fork_id*** fn emits ***trace*** event](crates/net/network/src/peers.rs#L682)
    [***PeersManager::add_peer_kind*** fn emits ***trace*** event](crates/net/network/src/peers.rs#L712)
    [***PeersManager::remove_peer*** fn emits ***trace*** event](crates/net/network/src/peers.rs#L752)
    [***PeersManager::add_and_connect_kind*** fn emits ***trace*** event](crates/net/network/src/peers.rs#L791)
    [***PeersManager::fill_outbound_slots*** fn emits ***trace*** event](crates/net/network/src/peers.rs#L876)
    [***NetworkState::ban_ip_discovery*** fn emits ***trace*** event](crates/net/network/src/state.rs#L282)
    [***NetworkState::ban_discovery*** fn emits ***trace*** event](crates/net/network/src/state.rs#L288)
    [***NetworkState::poll*** fn emits ***debug*** event](crates/net/network/src/state.rs#L421)
    [***Swarm::on_session_event*** fn emits ***trace*** event](crates/net/network/src/swarm.rs#L113)
    [***Swarm::on_connection*** fn emits ***trace*** events](crates/net/network/src/swarm.rs#L182)
    [***Swarm::on_state_action*** fn emits ***trace*** events](crates/net/network/src/swarm.rs#L230)

24. **reth-network-types**
    [***PeersConfig::with_basic_nodes_from_file*** fn emits ***info*** event](crates/net/network-types/src/peers/config.rs#L279)
    [***Peer::apply_reputation*** fn emits ***trace*** event](crates/net/network-types/src/peers/mod.rs#L86)

25. **reth-network-p2p**
    [***FetchFullBlockFuture::take_block*** fn emits ***debug*** event](crates/net/p2p/src/full_block.rs#L143)
    [***FetchFullBlockFuture::on_block_response*** fn emits ***debug*** event](crates/net/p2p/src/full_block.rs#L167)
    [***FetchFullBlockFuture::poll*** fn emits ***debug*** events](crates/net/p2p/src/full_block.rs#L187)
    [***FetchFullBlockFuture::take_blocks*** fn emits ***debug*** events](crates/net/p2p/src/full_block.rs#L391)
    [***FetchFullBlockFuture::on_headers_response*** fn emits ***debug*** events](crates/net/p2p/src/full_block.rs#L448)
    [***FetchFullBlockRangeFuture::poll*** fn emits ***debug*** events](crates/net/p2p/src/full_block.rs#L510)

26. **reth-node-builder**
    [***BuilderContext::start_network_with*** fn emits ***tace, warn, info*** events](crates/node/builder/src/builder/mod.rs#L675)
    [***LaunchContext::load_toml_config*** fn emits ***info*** events](crates/node/builder/src/launch/common.rs#L102)
    [***LaunchContext::save_pruning_config_if_full_node*** fn emits ***info, warn*** events](crates/node/builder/src/launch/common.rs#L122)
    [***LaunchContext::configure_globals*** fn emits ***debug, error*** events](crates/node/builder/src/launch/common.rs#L192)
    [***LaunchContextWith::with_resolved_peers*** fn emits ***info*** event](crates/node/builder/src/launch/common.rs#L227)
    [***LaunchContextWith::create_provider_factory*** fn emits ***info*** event](crates/node/builder/src/launch/common.rs#L383)
    [***LaunchContextWith::start_prometheus_endpoint*** fn emits ***info*** event](crates/node/builder/src/launch/common.rs#L503)
    [***LaunchContextWith::with_metrics_task*** fn emits ***debug*** event](crates/node/builder/src/launch/common.rs#L560)
    [***LaunchContextWith::with_components*** fn emits ***debug*** events](crates/node/builder/src/launch/common.rs#L659)
    [***LaunchContextWith::check_pipeline_consistency*** fn emits ***debug*** event](crates/node/builder/src/launch/common.rs#L822)
    [***EngineNodeLauncher::launch_node*** fn emits ***info, debug, error*** events](crates/node/builder/src/launch/engine.rs#L92)
    [***ExExLauncher::launch*** fn emits ***info, debug*** events](crates/node/builder/src/launch/exex.rs#L45)
    [***DefaultNodeLauncher::launch_node*** fn emits ***info, debug*** events](crates/node/builder/src/launch/mod.rs#L111)
    [***RpcAddOns::launch_add_ons*** fn emits ***info, debug*** events](crates/node/builder/src/rpc.rs#L416)
    [***build_pipeline*** fn emits ***debug*** event](crates/node/builder/src/setup.rs#L74)

27. **reth-node-core**
    [***NetworkArgs::resolved_addr*** fn emits ***error*** event](crates/node/core/src/args/network.rs#L161)
    [***NodeConfig::lookup_or_fetch_tip*** fn emits ***info*** event](crates/node/core/src/node_config.rs#L332)
    [***NodeConfig::fetch_tip_from_network*** fn emits ***info, error*** events](crates/node/core/src/node_config.rs#L356)
    [***get_or_create_jwt_secret_from_path*** fn emits ***info, debug*** events](crates/node/core/src/utils.rs#L27)
    [***get_single_header*** fn emits ***info, debug*** events](crates/node/core/src/utils.rs#L38)

28. **reth-node-events**
    [***NodeState::handle_pipeline_event*** fn emits ***info*** events](crates/node/events/src/node.rs#L88)
    [***NodeState::handle_consensus_engine_event*** fn emits ***info*** events](crates/node/events/src/node.rs#L214)
    [***NodeState::handle_consensus_layer_health_event*** fn emits ***warn*** events](crates/node/events/src/node.rs#L277)
    [***NodeState::handle_pruner_event*** fn emits ***info*** events](crates/node/events/src/node.rs#L298)
    [***NodeState::handle_static_file_producer_event*** fn emits ***info*** events](crates/node/events/src/node.rs#313)
    [***EventHandler::poll*** fn emits ***info, warn*** events](crates/node/events/src/node.rs#435)
    [***Eta::update*** fn emits ***debug*** events](crates/node/events/src/node.rs#557)

29. **reth-node-metrics**
    [***Hooks::new*** fn emits ***error*** event](crates/node/metrics/src/hooks.rs#25)
    [***collect_memory_stats*** fn emits ***error*** events](crates/node/metrics/src/hooks.rs#89)
    [***collect_io_stats*** fn emits ***error*** events](crates/node/metrics/src/hooks.rs#140)
    [***MetricServer::start_endpoint*** fn emits ***debug, error*** events](crates/node/metrics/src/server.rs#77)

30. **op-reth**
    [***main*** fn emits ***warn*** event](crates/optimism/bin/src/main.rs#16)

31. **reth-optimism-cli**
    [***ImportReceiptsOpCommand::execute*** fn emits ***info, debug*** events](crates/optimism/cli/src/commands/import_receipts.rs#L50)
    [***import_receipts_from_file*** fn emits ***trace, info*** events](crates/optimism/cli/src/commands/import_receipts.rs#L81)
    [***import_receipts_from_reader*** fn emits ***warn, info*** events](crates/optimism/cli/src/commands/import_receipts.rs#L123)
    [***ImportReceiptsOpCommand::execute*** fn emits ***info, debug*** events](crates/optimism/cli/src/commands/import_receipts.rs#L50)
    [***InitStateCommandOp::execute*** fn emits ***info*** events](crates/optimism/cli/src/commands/init_state.rs#L38)
    [***Cli::run*** fn emits ***info*** event](crates/optimism/cli/src/lib.rs#L132)

32. **reth-optimism-consensus**
    [***validate_block_post_execution*** fn emits ***debug*** events](crates/optimism/consensus/src/validation.rs#L12)

33. **reth-optimism-evm**
    [***OpExecutionStrategy::execute_transactions*** fn emits ***trace*** event](crates/optimism/evm/src/execute.rs#L162)

34. **reth-optimism-node**
    [***OpPoolBuilder::build_pool*** fn emits ***info, debug*** events](crates/optimism/node/src/node.rs#L393)
    [***OpNetworkBuilder::build_network*** fn emits ***info*** event](crates/optimism/node/src/node.rs#L640)

35. **reth-optimism-payload-builder**
    [***OpBuilder::build*** fn emits ***warn, debug***](crates/optimism/payload/src/builder.rs#L343)
    [***OpPayloadBuilderCtx::ensure_create2_deployer*** fn emits ***warn*** events](crates/optimism/payload/src/builder.rs#L691)
    [***OpPayloadBuilderCtx::apply_pre_beacon_root_contract_call*** fn emits ***warn*** events](crates/optimism/payload/src/builder.rs#L602)
    [***OpPayloadBuilderCtx::execute_sequencer_transactions*** fn emits ***trace*** events](crates/optimism/payload/src/builder.rs#L713)
    [***OpPayloadBuilderCtx::execute_best_transactions*** fn emits ***trace*** events](crates/optimism/payload/src/builder.rs#L842)

36. **reth-optimism-rpc**
    [***OpEthApi::send_raw_transaction*** fn emits ***debug*** events](crates/optimism/rpc/src/eth/transaction.rs#L34)
    [***SequencerClient::forward_raw_transaction*** fn emits ***warn*** events](crates/optimism/rpc/src/sequencer.rs#L54)

37. **reth-payload-builder**
    [***PayloadBuilderService::resolve*** fn emits ***trace*** events](crates/payload/builder/src/service.rs#L294)
    [***PayloadBuilderService::payload_attributes*** fn emits ***trace*** event](crates/payload/builder/src/service.rs#L337)
    [***PayloadBuilderService::poll*** fn emits ***trace, info, warn, debug*** events](crates/payload/builder/src/service.rs#L366)

38. **reth-prune**
    [Instrumented fn ***AccountHistory::prune*** at ***trace*** level. It also emits ***trace*** events](crates/prune/prune/src/segments/user/account_history.rs#L50)
    [Instrumented fn ***Receipts::prune*** at ***trace*** level](crates/prune/prune/src/segments/user/receipts.rs#L46)
    [Instrumented fn ***SenderRecovery::prune*** at ***trace*** level. It also emits ***trace*** events](crates/prune/prune/src/segments/user/sender_recovery.rs#L41)
    [Instrumented fn ***TransactionLookup::prune*** at ***trace*** level. It also emits ***trace*** events](crates/prune/prune/src/segments/user/transaction_lookup.rs#L41)
    [***Headers::prune*** fn emits ***trace*** event](crates/prune/prune/src/segments/static_file/headers.rs#L53)
    [***Transactions::prune*** fn emits ***trace*** events](crates/prune/prune/src/segments/static_file/transactions.rs#L50)
    [***prune*** fn emits ***trace*** events](crates/prune/prune/src/segments/receipts.rs#L18)
    [***PruneInput::get_next_tx_num_range*** fn emits ***error*** event](crates/prune/prune/src/segments/mod.rs#L75)
    [***DbTxPruneExt::prune_table_with_iterator*** fn emits ***debug*** event](crates/prune/prune/src/db_ext.rs#L16)
    [***Pruner::run_with_provider*** fn emits ***debug*** events](crates/prune/prune/src/pruner.rs#L115)
    [***Pruner::prune_segments*** fn emits ***debug*** events](crates/prune/prune/src/pruner.rs#L177)
    [***Pruner::is_pruning_needed*** fn emits ***debug*** event](crates/prune/prune/src/pruner.rs#L268)
    [***Pruner::adjust_tip_block_number_to_finished_exex_height*** fn emits ***debug*** event](crates/prune/prune/src/pruner.rs#L300)

40. **reth-ipc**
    [Instrumented fn ***process_batch_request*** at ***trace*** level](crates/rpc/ipc/src/server/ipc.rs#L35)
    [Instrumented fn ***execute_call_with_tracing*** at ***trace*** level](crates/rpc/ipc/src/server/ipc.rs#L106)
    [Instrumented fn ***execute_notification*** at ***trace*** level](crates/rpc/ipc/src/server/ipc.rs#L117)
    [Instrumented fn ***EngineEthApi::call*** at ***info*** level](crates/rpc/rpc/src/engine.rs#L70)
    [***Sender::send_ping*** fn emits ***debug*** event](crates/rpc/ipc/src/client/mod.rs#L33)
    [***IpcConnDriver::poll*** fn emits ***warn, debug*** events](crates/rpc/ipc/src/server/connection.rs#L91)
    [***RpcService::call*** fn emits ***warn*** events](crates/rpc/ipc/src/server/rpc_service.rs#L54)
    [***EthFilter::clear_stale_filters*** fn emits ***trace*** events](crates/rpc/rpc/src/eth/filter.rs#L122)
    [***EthFilter::new_filter*** fn emits ***trace*** event](crates/rpc/rpc/src/eth/filter.rs#L248)
    [***EthFilter::new_block_filter*** fn emits ***trace*** event](crates/rpc/rpc/src/eth/filter.rs#L256)
    [***EthFilter::new_pending_transaction_filter*** fn emits ***trace*** event](crates/rpc/rpc/src/eth/filter.rs#L262)
    [***EthFilter::filter_changes*** fn emits ***trace*** event](crates/rpc/rpc/src/eth/filter.rs#L293)
    [***EthFilter::filter_logs*** fn emits ***trace*** event](crates/rpc/rpc/src/eth/filter.rs#L306)
    [***EthFilter::uninstall_filter*** fn emits ***trace*** events](crates/rpc/rpc/src/eth/filter.rs#L312)
    [***EthFilter::logs*** fn emits ***trace*** event](crates/rpc/rpc/src/eth/filter.rs#L326)
    [***EthFilterInner::get_logs_in_block_range*** fn emits ***trace*** event](crates/rpc/rpc/src/eth/filter.rs#L462)
    [***handle_accepted*** fn emits ***error*** event](crates/rpc/rpc/src/eth/pubsub.rs#L90)
    [***EthSimBundle::sim_bundle*** fn emits ***info*** event](crates/rpc/rpc/src/eth/sim_bundle.rs#L448)
    [***TxPoolApi::txpool_status*** fn emits ***trace*** event](crates/rpc/rpc/src/txpool.rs#L84)
    [***TxPoolApi::txpool_inspect*** fn emits ***trace*** event](crates/rpc/rpc/src/txpool.rs#L96)
    [***TxPoolApi::txpool_content_from*** fn emits ***trace*** event](crates/rpc/rpc/src/txpool.rs#L136)
    [***TxPoolApi::txpool_content*** fn emits ***trace*** event](crates/rpc/rpc/src/txpool.rs#L149)

41. **reth-rpc-builder**
    [***TxPoolApi::txpool_content*** fn emits ***trace*** event](crates/rpc/rpc/src/txpool.rs#L149)
    [***RpcServerArgs::rpc_server_config*** fn emits ***warn*** event](crates/rpc/rpc-builder/src/config.rs#L176)
    [***RpcServerArgs::auth_jwt_secret*** fn emits ***debug*** event](crates/rpc/rpc-builder/src/config.rs#L220)

42. **reth-rpc-engine-api**
    [***EngineApi::get_payload_v1*** fn emits ***warn*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L372)
    [***EngineApi::get_payload_v2*** fn emits ***warn*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L396)
    [***EngineApi::get_payload_v3*** fn emits ***warn*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L431)
    [***EngineApi::get_payload_v4*** fn emits ***warn*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L466)
    [***EngineApi::new_payload_v1*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L764)
    [***EngineApi::new_payload_v2*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L771)
    [***EngineApi::new_payload_v3*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L778)
    [***EngineApi::new_payload_v4*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L790)
    [***EngineApi::fork_choice_updated_v1*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L812)
    [***EngineApi::fork_choice_updated_v2*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L827)
    [***EngineApi::fork_choice_updated_v3*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L843)
    [***EngineApi::get_payload_v1*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L867)
    [***EngineApi::get_payload_v2*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L887)
    [***EngineApi::get_payload_v3*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L907)
    [***EngineApi::get_payload_v4*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L927)
    [***EngineApi::get_payload_bodies_by_hash_v1*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L940)
    [***EngineApi::get_payload_bodies_by_range_v1*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L967)
    [***EngineApi::exchange_transition_configuration*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L981)
    [***EngineApi::get_client_version_v1*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L995)
    [***EngineApi::get_blobs_v1*** fn emits ***trace*** event](crates/rpc/rpc-engine-api/src/engine_api.rs#L1011)

43. **reth-rpc-eth-types**
    [***RpcInvalidTransactionError::from*** fn emits ***error*** event](crates/rpc/rpc-eth-types/src/error/mod.rs#L494)
    [***fee_history_cache_new_blocks_task*** fn emits ***trace*** event](crates/rpc/rpc-eth-types/src/fee_history.rs#L210)
    [***GasPriceOracle::new*** fn emits ***warn*** event](crates/rpc/rpc-eth-types/src/gas_oracle.rs#L95)
    [***JwtAuthValidator::validate*** fn emits ***error*** events](crates/rpc/rpc-layer/src/jwt_validator.rs#L24)

44. **reth-stages-api**
    [***MetricsListener::handle_event*** fn emits ***trace*** event](crates/stages/api/src/metrics/listener.rs#L48)
    [***Pipeline::set_tip*** fn emits ***warn*** event](crates/stages/api/src/pipeline/mod.rs#L101)
    [***Pipeline::run_as_fut*** fn emits ***trace*** event](crates/stages/api/src/pipeline/mod.rs#L133)
    [***Pipeline::run*** fn emits ***trace*** event](crates/stages/api/src/pipeline/mod.rs#L164)
    [***Pipeline::run_loop*** fn emits ***trace*** events](crates/stages/api/src/pipeline/mod.rs#L205)
    [***Pipeline::unwind*** fn emits ***info, debug*** events](crates/stages/api/src/pipeline/mod.rs#L278)
    [***Pipeline::execute_stage_to_completion*** fn emits ***info, debug*** events](crates/stages/api/src/pipeline/mod.rs#L379)
    [***on_stage_error*** fn emits ***warn, error*** events](crates/stages/api/src/pipeline/mod.rs#L497)

45. **reth-stages**
    [***BodyStage::execute*** fn emits ***trace, debug*** events](crates/stages/stages/src/stages/bodies.rs#L193)
    [***ExecutionStage::execute*** fn emits ***trace, info, debug*** events](crates/stages/stages/src/stages/execution.rs#L296)
    [***ExecutionStage::unwind*** fn emits ***debug*** events](crates/stages/stages/src/stages/execution.rs#L479)
    [***calculate_gas_used_from_headers*** fn emits ***debug*** events](crates/stages/stages/src/stages/execution.rs#L620)
    [***AccountHashingStage::execute*** fn emits ***info*** event](crates/stages/stages/src/stages/hashing_account.rs#L144)
    [***collect*** fn emits ***info*** event](crates/stages/stages/src/stages/hashing_account.rs#L255)
    [***StorageHashingStage::execute*** fn emits ***info*** event](crates/stages/stages/src/stages/hashing_storage.rs#L75)
    [***collect*** fn emits ***info*** event](crates/stages/stages/src/stages/hashing_storage.rs#L187)
    [***HeaderStage::write_headers*** fn emits ***info*** events](crates/stages/stages/src/stages/headers.rs#L93)
    [***HeaderStage::poll_execute_ready*** fn emits ***info, debug, error*** events](crates/stages/stages/src/stages/headers.rs#L211)
    [***IndexAccountHistoryStage::execute*** fn emits ***info*** events](crates/stages/stages/src/stages/index_account_history.rs#L56)
    [***IndexStorageHistoryStage::execute*** fn emits ***info*** events](crates/stages/stages/src/stages/index_storage_history.rs#L58)
    [***MerkleStage::save_execution_checkpoint*** fn emits ***debug*** event](crates/stages/stages/src/stages/merkle.rs#L116)
    [***MerkleStage::unwind*** fn emits ***info*** event](crates/stages/stages/src/stages/merkle.rs#L290)
    [***validate_state_root*** fn emits ***error*** event](crates/stages/stages/src/stages/merkle.rs#L347)
    [***PruneStage::execute*** fn emits ***info*** events](crates/stages/stages/src/stages/prune.rs#L51)
    [***SenderRecoveryStage::execute*** fn emits ***info*** events](crates/stages/stages/src/stages/sender_recovery.rs#L76)
    [***recover_range*** fn emits ***debug*** events](crates/stages/stages/src/stages/sender_recovery.rs#L142)
    [***TransactionLookupStage::execute*** fn emits ***trace, info*** events](crates/stages/stages/src/stages/tx_lookup.rs#L75)
    [***TransactionLookupStage::unwind*** fn emits ***trace, info*** events](crates/stages/stages/src/stages/tx_lookup.rs#L190)
    [***collect_history_indices*** fn emits ***info*** events](crates/stages/stages/src/stages/utils.rs#L42)
    [***load_history_indices*** fn emits ***info*** events](crates/stages/stages/src/stages/utils.rs#L109)

46. **reth-static-file**
    [***StaticFileProducerInner::run*** fn emits ***debug*** events](crates/static-file/static-file/src/static_file_producer.rs#L104)
    [***DatabaseEnv::open*** fn emits ***warn*** event](crates/storage/db/src/implementation/mdbx/mod.rs#L283)
    
47. **reth-db**
    [***Tx::execute_with_close_transaction_metric*** fn emits ***debug*** event](crates/storage/db/src/implementation/mdbx/tx.rs#L102)
    [***MetricsHandler::log_transaction_opened*** fn emits ***trace*** event](crates/storage/db/src/implementation/mdbx/tx.rs#L218)[***MetricsHandler::log_backtrace_on_long_read_transaction*** fn emits ***warn*** event](crates/storage/db/src/implementation/mdbx/tx.rs#L234)
    [***StorageLock::try_acquire_file_lock*** fn emits ***error*** event](crates/storage/db/src/lockfile.rs#L46)
    [***StorageLock::drop*** fn emits ***error*** event](crates/storage/db/src/lockfile.rs#L66)

48. **reth-db-common**
    [***DbTool::drop*** fn emits ***info*** events](crates/storage/db-common/src/db_tool/mod.rs#L135)
    [***init_genesis*** fn emits ***debug*** events](crates/storage/db-common/src/init.rs#L76)
    [***insert_state*** fn emits ***error, trace*** events](crates/storage/db-common/src/init.rs#L161)
    [***insert_genesis_hashes*** fn emits ***trace*** events](crates/storage/db-common/src/init.rs#L261)
    [***insert_history*** fn emits ***trace*** events](crates/storage/db-common/src/init.rs#L299)
    [***init_from_state_dump*** fn emits ***info, debug, error*** events](crates/storage/db-common/src/init.rs#L357)
    [***parse_state_root*** fn emits ***trace*** event](crates/storage/db-common/src/init.rs#L440)
    [***parse_accounts*** fn emits ***info*** event](crates/storage/db-common/src/init.rs#L453)
    [***dump_state*** fn emits ***info*** event](crates/storage/db-common/src/init.rs#L482)
    [***compute_state_root*** fn emits ***trace, info*** event](crates/storage/db-common/src/init.rs#L544)

49. **reth-nippy-jar**
    [***Zstd::compressors*** fn emits ***debug*** event](crates/storage/nippy-jar/src/compression/zstd.rs#L74)
    [***Zstd::compress_with_dictionary*** fn emits ***debug*** event](crates/storage/nippy-jar/src/compression/zstd.rs#L93)
    [***NippyJar::prepare_compression*** fn emits ***debug*** event](crates/storage/nippy-jar/src/lib.rs#L260)
    [***NippyJar::freeze*** fn emits ***debug*** events](crates/storage/nippy-jar/src/lib.rs#L273)

50. **reth-provider**
    [***ProviderFactory::latest*** fn emits ***trace*** event](crates/storage/provider/src/providers/database/mod.rs#L182)
    [***ProviderFactory::history_by_block_number*** fn emits ***trace*** event](crates/storage/provider/src/providers/database/mod.rs#L188)
    [***ProviderFactory::history_by_block_hash*** fn emits ***trace*** event](crates/storage/provider/src/providers/database/mod.rs#L198)
    [***DatabaseProvider::latest*** fn emits ***trace*** event](crates/storage/provider/src/providers/database/provider.rs#L166)
    [***DatabaseProvider::write_state_reverts*** fn emits ***trace*** events](crates/storage/provider/src/providers/database/provider.rs#L1883)
    [***DatabaseProvider::write_state_changes*** fn emits ***trace*** events](crates/storage/provider/src/providers/database/provider.rs#L1951)
    [***DatabaseProvider::insert_hashes*** fn emits ***debug*** event](crates/storage/provider/src/providers/database/provider.rs#L2526)
    [***DatabaseProvider::insert_block*** fn emits ***warn, debug*** event](crates/storage/provider/src/providers/database/provider.rs#L2813)
    [***DatabaseProvider::append_blocks_with_state*** fn emits ***debug*** event](crates/storage/provider/src/providers/database/provider.rs#L3057)
    [***HistoricalStateProviderRef::revert_state*** fn emits ***warn*** event](crates/storage/provider/src/providers/state/historical.rs#L124)
    [***HistoricalStateProviderRef::revert_storage*** fn emits ***warn*** event](crates/storage/provider/src/providers/state/historical.rs#L145)
    [***StaticFileProvider::watch_directory*** fn emits ***warn, info*** event](crates/storage/provider/src/providers/static_file/manager.rs#L128)
    [***StaticFileProvider::get_or_create_jar_provider*** fn emits ***trace*** events](crates/storage/provider/src/providers/static_file/manager.rs#L442)
    [***StaticFileProvider::check_consistency*** fn emits ***info*** events](crates/storage/provider/src/providers/static_file/manager.rs#L651)
    [***StaticFileProvider::ensure_invariants*** fn emits ***info*** events](crates/storage/provider/src/providers/static_file/manager.rs#L823)
    [***StaticFileProvider::fetch_range_with_predicate*** fn emits ***warn*** event](crates/storage/provider/src/providers/static_file/manager.rs#L967)
    [***StaticFileProvider::get_writer*** fn emits ***trace*** event](crates/storage/provider/src/providers/static_file/manager.rs#L1191)
    [***StaticFileProviderRW::commit*** fn emits ***debug*** event](crates/storage/provider/src/providers/static_file/writer.rs#L221)
    [***StaticFileProviderRW::commit_without_sync_all*** fn emits ***debug*** event](crates/storage/provider/src/providers/static_file/writer.rs#L266)
    [***BlockchainProvider2::latest*** fn emits ***trace*** events](crates/storage/provider/src/providers/blockchain_provider.rs#L557)
    [***BlockchainProvider2::history_by_block_number*** fn emits ***trace*** event](crates/storage/provider/src/providers/blockchain_provider.rs#L569)
    [***BlockchainProvider2::history_by_block_hash*** fn emits ***trace*** event](crates/storage/provider/src/providers/blockchain_provider.rs#L582)
    [***BlockchainProvider2::state_by_block_hash*** fn emits ***trace*** event](crates/storage/provider/src/providers/blockchain_provider.rs#L595)
    [***BlockchainProvider2::pending*** fn emits ***trace*** event](crates/storage/provider/src/providers/blockchain_provider.rs#L613)
    [***ConsistentProvider::latest_ref*** fn emits ***trace*** events](crates/storage/provider/src/providers/consistent.rs#L109)
    [***ConsistentProvider::history_by_block_hash_ref*** fn emits ***trace*** event](crates/storage/provider/src/providers/consistent.rs#L122)
    [***BlockchainProvider::pending_with_provider*** fn emits ***trace*** event](crates/storage/provider/src/providers/mod.rs#L222)
    [***BlockchainProvider::latest*** fn emits ***trace*** event](crates/storage/provider/src/providers/mod.rs#L655)
    [***BlockchainProvider::history_by_block_number*** fn emits ***trace*** event](crates/storage/provider/src/providers/mod.rs#L692)
    [***BlockchainProvider::history_by_block_hash*** fn emits ***trace*** event](crates/storage/provider/src/providers/mod.rs#L701)
    [***BlockchainProvider::state_by_block_hash*** fn emits ***trace*** event](crates/storage/provider/src/providers/mod.rs#L706)
    [***BlockchainProvider::pending*** fn emits ***trace*** event](crates/storage/provider/src/providers/mod.rs#L725)
    [***UnifiedStorageWriter::save_blocks*** fn emits ***debug*** events](crates/storage/provider/src/writer/mod.rs#L135)
    [***UnifiedStorageWriter::remove_blocks_above*** fn emits ***debug*** events](crates/storage/provider/src/writer/mod.rs#L197)

51. **reth-tasks**
    [***TaskManager::do_graceful_shutdown*** fn emits ***debug*** events](crates/tasks/src/lib.rs#L227)   

52. **reth-tokio-util**
    [***EventSender::notify*** fn emits ***trace*** event](crates/tokio-util/src/event_sender.rs#L31)
    [***EventStream::poll_next*** fn emits ***warn*** event](crates/tokio-util/src/event_stream.rs#L33)
    [***DiskFileBlobStore::cleanup*** fn emits ***debug*** events](crates/transaction-pool/src/blobstore/disk.rs#L77)
    [***DiskFileBlobStoreInner::create_blob_dir*** fn emits ***debug*** event](crates/transaction-pool/src/blobstore/disk.rs#L185)
    [***DiskFileBlobStoreInner::delete_all*** fn emits ***debug*** event](crates/transaction-pool/src/blobstore/disk.rs#L192)
    [***DiskFileBlobStoreInner::insert_many*** fn emits ***debug*** event](crates/transaction-pool/src/blobstore/disk.rs#L216)
    [***DiskFileBlobStoreInner::read_many_raw*** fn emits ***debug*** event](crates/transaction-pool/src/blobstore/disk.rs#L338)
    [***DiskFileBlobStoreInner::write_one_encoded*** fn emits ***trace*** event](crates/transaction-pool/src/blobstore/disk.rs#L357)
    [***BestTransactions::next*** fn emits ***debug*** event](crates/transaction-pool/src/pool/best.rs#L187)

53. **reth-transaction-pool**
    [Instrumented fn ***Pool::set_block_info*** at ***trace*** level. It also emits ***trace*** event](crates/transaction-pool/src/lib.rs#L612)
    [***PoolInner::get_pooled_transaction_elements*** fn emits ***debug*** event](crates/transaction-pool/src/pool/mod.rs#L341)
    [***PoolInner::on_canonical_state_change*** fn emits ***trace*** event](crates/transaction-pool/src/pool/mod.rs#L381)
    [***PoolInner::add_transaction*** fn emits ***debug*** events](crates/transaction-pool/src/pool/mod.rs#L428)
    [***PoolInner::on_new_blob_sidecar*** fn emits ***debug*** events](crates/transaction-pool/src/pool/mod.rs#L612)
    [***PoolInner::notify_on_new_state*** fn emits ***trace*** event](crates/transaction-pool/src/pool/mod.rs#L639)
    [***PoolInner::insert_blob*** fn emits ***debug*** event](crates/transaction-pool/src/pool/mod.rs#L914)
    [***PendingTransactionHashListener::send_all*** fn emits ***debug*** event](crates/transaction-pool/src/pool/mod.rs#L979)
    [***TransactionListener::send_all*** fn emits ***debug*** event](crates/transaction-pool/src/pool/mod.rs#L1020)
    [***TxPool::process_updates*** fn emits ***trace, debug*** events](crates/transaction-pool/src/pool/txpool.rs#L684)
    [***TxPool::remove_from_subpool*** fn emits ***trace*** event](crates/transaction-pool/src/pool/txpool.rs#L809)
    [***TxPool::prune_from_subpool*** fn emits ***trace*** event](crates/transaction-pool/src/pool/txpool.rs#L833)
    [***TxPool::add_transaction_to_subpool*** fn emits ***trace*** event](crates/transaction-pool/src/pool/txpool.rs#L881)
    [***TxPool::discard_worst*** fn emits ***trace*** events](crates/transaction-pool/src/pool/txpool.rs#L922)
    [***maintain_transaction_pool*** fn emits ***trace, debug*** events](crates/transaction-pool/src/maintain.rs#L95)
    [***load_and_reinsert_transactions*** fn emits ***info, debug*** events](crates/transaction-pool/src/maintain.rs#L554)
    [***save_local_txs_backup*** fn emits ***info, trace, warn*** events](crates/transaction-pool/src/maintain.rs#L591)
    [***backup_local_transactions_task*** fn emits ***error*** event](crates/transaction-pool/src/maintain.rs#L638)

54. **reth-trie-db**
    [***DatabaseStateRoot::incremental_root*** fn emits ***debug*** event](crates/trie/db/src/state.rs#L144)
    [***DatabaseStateRoot::incremental_root_with_updates*** fn emits ***debug*** event](crates/trie/db/src/state.rs#L152)
    [***DatabaseStateRoot::incremental_root_with_progress*** fn emits ***debug*** event](crates/trie/db/src/state.rs#L160)
    [***ParallelProof::multiproof*** fn emits ***debug*** event](crates/trie/parallel/src/proof.rs#L74)
    [***ParallelStateRoot::calculate*** fn emits ***trace, debug*** events](crates/trie/parallel/src/root.rs#L81)
    [***RevealedSparseTrie::remove_leaf*** fn emits ***debug*** events](crates/trie/sparse/src/trie.rs#L923)
    [***StateRoot::calculate*** fn emits ***trace*** events](crates/trie/trie/src/trie.rs#L151)
    [***StorageRoot::calculate*** fn emits ***trace*** events](crates/trie/trie/src/trie.rs#L394)
