---
title: backfill sync failed err=Cannot unwind to block 8523798 as it is beyond the AccountHistory limit
labels:
    - C-bug
    - S-needs-triage
    - S-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.992804Z
info:
    author: gemcoder21
    created_at: 2025-07-09T16:54:30Z
    updated_at: 2026-01-05T02:22:28Z
---

### Describe the bug

Not able to sync full mode from scratch, no extra configuration is done, just simple full node

```
2025-07-08T18:11:34.667945Z  INFO Preparing stage pipeline_stages=1/15 stage=Era checkpoint=0 target=None
2025-07-08T18:11:34.667964Z  INFO Executing stage pipeline_stages=1/15 stage=Era checkpoint=0 target=None
2025-07-08T18:11:34.667971Z  INFO Finished stage pipeline_stages=1/15 stage=Era checkpoint=0 target=None
2025-07-08T18:11:34.668002Z  INFO Preparing stage pipeline_stages=2/15 stage=Headers checkpoint=22876083 target=0
2025-07-08T18:11:34.878763Z  INFO Received headers total=32 from_block=22876115 to_block=22876084
2025-07-08T18:11:34.878880Z  INFO Writing headers total=32
2025-07-08T18:11:34.878916Z  INFO Executing stage pipeline_stages=2/15 stage=Headers checkpoint=22876083 target=0
2025-07-08T18:11:34.879943Z  INFO Writing headers hash index total=32
2025-07-08T18:11:34.885406Z  INFO Finished stage pipeline_stages=2/15 stage=Headers checkpoint=22876115 target=0 stage_progress=100.00%
2025-07-08T18:11:34.896745Z  INFO Preparing stage pipeline_stages=3/15 stage=Bodies checkpoint=22876083 target=22876115
2025-07-08T18:11:34.896751Z  INFO Downloading bodies count=32 range=22876084..=22876115
2025-07-08T18:11:35.512060Z  INFO Executing stage pipeline_stages=3/15 stage=Bodies checkpoint=22876083 target=22876115
2025-07-08T18:11:35.532749Z  INFO Finished stage pipeline_stages=3/15 stage=Bodies checkpoint=22876115 target=22876115 stage_progress=100.00%
2025-07-08T18:11:35.537032Z  INFO Preparing stage pipeline_stages=4/15 stage=SenderRecovery checkpoint=22876083 target=22876115
2025-07-08T18:11:35.537056Z  INFO Recovering senders tx_range=2881861920..2881868608
2025-07-08T18:11:35.537059Z  INFO Executing stage pipeline_stages=4/15 stage=SenderRecovery checkpoint=22876083 target=22876115
2025-07-08T18:11:35.564694Z  INFO Finished stage pipeline_stages=4/15 stage=SenderRecovery checkpoint=22876115 target=22876115 stage_progress=100.00%
2025-07-08T18:11:35.565804Z  INFO Preparing stage pipeline_stages=5/15 stage=Execution checkpoint=8523798 target=22876115
2025-07-08T18:11:35.565853Z  INFO Executing stage pipeline_stages=5/15 stage=Execution checkpoint=8523798 target=22876115
2025-07-08T18:11:37.397300Z  INFO Received block from consensus engine number=22876179 hash=0x47cf97aeea074a607075b150e6a1bee53db4e6811ba90bf88a9ca557c7a39f19
2025-07-08T18:02:40.522254Z ERROR Stage encountered a validation error: block gas used mismatch: got 3407989, expected 7999018; gas spent by each transaction: [(0, 21000), (1, 59360), (2, 625713), (3, 662919), (4, 2014454), (5, 3365989), (6, 3386989), (7, 3407989)] stage=Execution bad_block=8524574
2025-07-08T18:02:40.522506Z ERROR backfill sync failed err=Cannot unwind to block 8523798 as it is beyond the AccountHistory limit. Latest block: 22876051, History limit: 10064
2025-07-08T18:02:40.522543Z ERROR Fatal error in consensus engine
2025-07-08T18:02:40.522592Z ERROR shutting down due to error
Error: Fatal error in consensus engine
Location:
    /home/runner/work/reth/reth/crates/node/builder/src/launch/engine.rs:318:43
````

Crash in consensus engine.rs:318:43

### Steps to reproduce

I'm running docker compose:
```
reth:
    image: ghcr.io/paradigmxyz/reth:v1.5.1
    container_name: reth
    volumes:
      - /mnt/chain_drive/reth-data:/root/.local/share/reth
      - ./jwtsecret:/jwtsecret
    ports:
      - "8545:8545"    # HTTP RPC for L1
      - "30303:30303"  # P2P
      - "8551:8551"    # Engine API
    command: >
      node
      --full
      --chain mainnet
      --http
      --http.addr=0.0.0.0
      --http.port=8545
      --http.api=eth,net,web3,trace,debug
      --http.corsdomain=*
      --authrpc.addr=0.0.0.0
      --authrpc.port=8551
      --authrpc.jwtsecret=/jwtsecret
      --port=30303
lighthouse:
    image: sigp/lighthouse:v7.0.1
    container_name: lighthouse
    volumes:
      - /mnt/chain_drive/lighthouse-data:/root/.lighthouse
      - ./jwtsecret:/jwtsecret
    ports:
      - "9000:9000"  # P2P
      - "5052:5052"  # Beacon API
    command: >
      lighthouse bn
      --network=mainnet
      --execution-endpoint=http://reth:8551
      --execution-jwt=/jwtsecret
      --checkpoint-sync-url=https://mainnet.checkpoint.sigp.io
      --http
      --http-address=0.0.0.0
      --http-port=5052
      --disable-deposit-contract-sync
    depends_on:
      reth:
        condition: service_started
    networks:
      - blockchain
    restart: unless-stopped
```

### Node logs

```text
2025-07-09T16:47:45.943062Z DEBUG net: Request canceled, response channel from session closed. id=0x33f740e98010f52840296c78b7f574cc5329507959f8871757b8c7b7b43185c49976d5f18fbdaf63d0311690201751fed6e63ae0b3bd8ed81b3e82eb79d9a203
2025-07-09T16:47:45.943099Z DEBUG ChainOrchestrator::poll: downloaders: Header range download failed err=connection to a peer dropped while handling the request this.start_hash=0x0a5011f07883f0dd29b4f96629bf6b1ef8898f786078b6702e9d2d5b6685b2df
2025-07-09T16:47:48.899825Z DEBUG engine::tree: received new engine message msg=Request(NewPayload(parent: 0x72de603d625eb122c46cd62e4c56f697d0f8156354b19dcdf44263b63ec896f8, number: 22882917, hash: 0x4965389dfcc5f5f516a1a1390f517bd21fc3d1cb19a61430a2a5ea40fbcc89e8))
2025-07-09T16:47:48.900547Z DEBUG engine::tree: Inserting new block into tree block=NumHash { number: 22882917, hash: 0x4965389dfcc5f5f516a1a1390f517bd21fc3d1cb19a61430a2a5ea40fbcc89e8 } parent=0x72de603d625eb122c46cd62e4c56f697d0f8156354b19dcdf44263b63ec896f8 state_root=0x8467dbcf06d607c0a3bc1788133f11638a3897a201fe66713d8f5d794f480dfa
2025-07-09T16:47:48.900611Z DEBUG reth::cli: Event: Handler(BlockReceived(NumHash { number: 22882917, hash: 0x4965389dfcc5f5f516a1a1390f517bd21fc3d1cb19a61430a2a5ea40fbcc89e8 }))
2025-07-09T16:47:48.900674Z  INFO reth_node_events::node: Received block from consensus engine number=22882917 hash=0x4965389dfcc5f5f516a1a1390f517bd21fc3d1cb19a61430a2a5ea40fbcc89e8
2025-07-09T16:47:48.901160Z DEBUG engine::tree: no canonical state found for block hash=0x72de603d625eb122c46cd62e4c56f697d0f8156354b19dcdf44263b63ec896f8
2025-07-09T16:47:49.030180Z DEBUG engine::tree: received new engine message msg=Request(ForkchoiceUpdated { state: ForkchoiceState { head_block_hash: 0x4965389dfcc5f5f516a1a1390f517bd21fc3d1cb19a61430a2a5ea40fbcc89e8, safe_block_hash: 0xb63c5d0449df0d4bbae63007b5f516387a7b25ecbf5ab70523079dee75235d9c, finalized_block_hash: 0x74d3b19599fec1ad223f223d9f59da013097557b6b2fe90240bb499d14ff3f10 }, has_payload_attributes: false })
2025-07-09T16:47:49.030240Z DEBUG engine::tree: New head block not found in inmemory tree state new_head=0x4965389dfcc5f5f516a1a1390f517bd21fc3d1cb19a61430a2a5ea40fbcc89e8
2025-07-09T16:47:49.030255Z DEBUG reth::cli: Event: Handler(ForkchoiceUpdated(ForkchoiceState { head_block_hash: 0x4965389dfcc5f5f516a1a1390f517bd21fc3d1cb19a61430a2a5ea40fbcc89e8, safe_block_hash: 0xb63c5d0449df0d4bbae63007b5f516387a7b25ecbf5ab70523079dee75235d9c, finalized_block_hash: 0x74d3b19599fec1ad223f223d9f59da013097557b6b2fe90240bb499d14ff3f10 }, Syncing))
2025-07-09T16:47:49.448509Z DEBUG engine::tree: received new engine message msg=DownloadedBlocks(1 blocks)
2025-07-09T16:47:49.448535Z DEBUG engine::tree: Inserting new block into tree block=NumHash { number: 22882869, hash: 0x22092f2d7db82a7fa240718f6b4993c8d210e78355619f28d253fe129da6cc6b } parent=0x823ba5d67f6dabcab2ae9b6ef304ae1098bfd7133f5749df456c80b1129fa92f state_root=0x44b5b8a8f773c8695aebb71433ad1e84ac662c9533e49c563364d829e194ceba
2025-07-09T16:47:49.449244Z DEBUG engine::tree: no canonical state found for block hash=0x823ba5d67f6dabcab2ae9b6ef304ae1098bfd7133f5749df456c80b1129fa92f
2025-07-09T16:47:49.734856Z DEBUG net::session: timed out outgoing request id=889 remote_peer_id=0x0990cee4b4182fdacf114aa7e2e00ac8666d6d1313cd2c98c0aa1259d6088cb8153a7980f86303711f437e3a2ede19f4d5e9271bbc7ef272ff9d7d5c0f1d40a2
2025-07-09T16:47:49.734876Z DEBUG net::peers: applied reputation change reputation=-4096 banned=false kind=Timeout
2025-07-09T16:47:49.758120Z DEBUG ChainOrchestrator::poll: downloaders: Header range download failed err=request timed out while awaiting response this.start_hash=0x4b7498bf5f3b4821ccea79be3ddf0efcf75508074f49f97589ef79a9adde583b
2025-07-09T16:47:56.555610Z DEBUG net: fork id mismatch, removing peer peer_id=0x1762e544b9bc8fb7cdf7a43afb6e2f120137d7c2972793a0711f85972679bac2ec108744159624d119019e4612b6fb3853f7a12086a27f0d5ac6670767aa7b2e remote_fork_id=ForkId { hash: ForkHash("49402b5c"), next: 0 } our_fork_id=ForkId { hash: ForkHash("fc64ec04"), next: 1150000 }
2025-07-09T16:47:57.209476Z DEBUG net: Session established remote_addr=40.160.13.217:30303 client_version=r peer_id=0x3db469f727c9cdca42d0b63922caabd21c683735629d053f0b16cd4d9ae4d6887e7ea2cb4b18b6238479c13d21998ae0e86487eed5a7ce2e30648c6e6aa03547 total_active=11 kind=outgoing peer_enode=enode://3db469f727c9cdca42d0b63922caabd21c683735629d053f0b16cd4d9ae4d6887e7ea2cb4b18b6238479c13d21998ae0e86487eed5a7ce2e30648c6e6aa03547@40.160.13.217:30303
2025-07-09T16:47:57.209538Z DEBUG net::session: failed to receive message err=disconnected remote_peer_id=0x3db469f727c9cdca42d0b63922caabd21c683735629d053f0b16cd4d9ae4d6887e7ea2cb4b18b6238479c13d21998ae0e86487eed5a7ce2e30648c6e6aa03547
2025-07-09T16:47:57.209576Z DEBUG net: Request canceled, response channel from session closed. id=0x3db469f727c9cdca42d0b63922caabd21c683735629d053f0b16cd4d9ae4d6887e7ea2cb4b18b6238479c13d21998ae0e86487eed5a7ce2e30648c6e6aa03547
2025-07-09T16:47:57.210750Z DEBUG ChainOrchestrator::poll: downloaders: Header range download failed err=connection to a peer dropped while handling the request this.start_hash=0x0a5011f07883f0dd29b4f96629bf6b1ef8898f786078b6702e9d2d5b6685b2df
2025-07-09T16:48:01.095729Z DEBUG engine::tree: received new engine message msg=Request(NewPayload(parent: 0x4965389dfcc5f5f516a1a1390f517bd21fc3d1cb19a61430a2a5ea40fbcc89e8, number: 22882918, hash: 0xf0f16d2dd233cec1af083a84e8b5f3f0165acb3bab35dbd6be9f01c76788252e))
2025-07-09T16:48:01.097557Z DEBUG engine::tree: Inserting new block into tree block=NumHash { number: 22882918, hash: 0xf0f16d2dd233cec1af083a84e8b5f3f0165acb3bab35dbd6be9f01c76788252e } parent=0x4965389dfcc5f5f516a1a1390f517bd21fc3d1cb19a61430a2a5ea40fbcc89e8 state_root=0x934103f30224e90e8086735fe1cc8dff7f58898e41f944762ae485189f2ef8a5
2025-07-09T16:48:01.097569Z DEBUG reth::cli: Event: Handler(BlockReceived(NumHash { number: 22882918, hash: 0xf0f16d2dd233cec1af083a84e8b5f3f0165acb3bab35dbd6be9f01c76788252e }))
2025-07-09T16:48:01.097608Z  INFO reth_node_events::node: Received block from consensus engine number=22882918 hash=0xf0f16d2dd233cec1af083a84e8b5f3f0165acb3bab35dbd6be9f01c76788252e
2025-07-09T16:48:01.109155Z DEBUG engine::tree: no canonical state found for block hash=0x4965389dfcc5f5f516a1a1390f517bd21fc3d1cb19a61430a2a5ea40fbcc89e8
2025-07-09T16:48:01.237494Z DEBUG engine::tree: received new engine message msg=Request(ForkchoiceUpdated { state: ForkchoiceState { head_block_hash: 0xf0f16d2dd233cec1af083a84e8b5f3f0165acb3bab35dbd6be9f01c76788252e, safe_block_hash: 0xb63c5d0449df0d4bbae63007b5f516387a7b25ecbf5ab70523079dee75235d9c, finalized_block_hash: 0x74d3b19599fec1ad223f223d9f59da013097557b6b2fe90240bb499d14ff3f10 }, has_payload_attributes: false })
2025-07-09T16:48:01.237568Z DEBUG engine::tree: New head block not found in inmemory tree state new_head=0xf0f16d2dd233cec1af083a84e8b5f3f0165acb3bab35dbd6be9f01c76788252e
2025-07-09T16:48:01.237611Z DEBUG reth::cli: Event: Handler(ForkchoiceUpdated(ForkchoiceState { head_block_hash: 0xf0f16d2dd233cec1af083a84e8b5f3f0165acb3bab35dbd6be9f01c76788252e, safe_block_hash: 0xb63c5d0449df0d4bbae63007b5f516387a7b25ecbf5ab70523079dee75235d9c, finalized_block_hash: 0x74d3b19599fec1ad223f223d9f59da013097557b6b2fe90240bb499d14ff3f10 }, Syncing))
2025-07-09T16:48:01.791366Z DEBUG engine::tree: received new engine message msg=DownloadedBlocks(1 blocks)
2025-07-09T16:48:01.791400Z DEBUG engine::tree: Inserting new block into tree block=NumHash { number: 22882868, hash: 0x823ba5d67f6dabcab2ae9b6ef304ae1098bfd7133f5749df456c80b1129fa92f } parent=0xe556a78745383dafc40643f711f2b28abf004979740b4cb0c3bac55d15d145ba state_root=0x27b456166518ac878c6b95121eaf203fdc079db0975afe85a3c6e11d1e8417f1
2025-07-09T16:48:01.792641Z DEBUG engine::tree: no canonical state found for block hash=0xe556a78745383dafc40643f711f2b28abf004979740b4cb0c3bac55d15d145ba
2025-07-09T16:48:04.090209Z  INFO reth::cli: Status connected_peers=10 latest_block=0
2025-07-09T16:48:07.717041Z DEBUG net: Session established remote_addr=40.160.16.181:30303 client_version=r peer_id=0xb6083ecc457da689ece677a3d66380f6db378c22429a7bcb231519ba388f2fb3b90244c39783ce0a358cb6286db1a8093428313540b3d20d928f67d4bfb25c05 total_active=11 kind=outgoing peer_enode=enode://b6083ecc457da689ece677a3d66380f6db378c22429a7bcb231519ba388f2fb3b90244c39783ce0a358cb6286db1a8093428313540b3d20d928f67d4bfb25c05@40.160.16.181:30303
2025-07-09T16:48:07.717109Z DEBUG net::session: failed to receive message err=disconnected remote_peer_id=0xb6083ecc457da689ece677a3d66380f6db378c22429a7bcb231519ba388f2fb3b90244c39783ce0a358cb6286db1a8093428313540b3d20d928f67d4bfb25c05
2025-07-09T16:48:07.717154Z DEBUG net: Request canceled, response channel from session closed. id=0xb6083ecc457da689ece677a3d66380f6db378c22429a7bcb231519ba388f2fb3b90244c39783ce0a358cb6286db1a8093428313540b3d20d928f67d4bfb25c05
2025-07-09T16:48:07.719187Z DEBUG ChainOrchestrator::poll: downloaders: Header range download failed err=connection to a peer dropped while handling the request this.start_hash=0xd066f24052368ca4a976a4c6aafbc937924dc925d0d242330070029b269cb4df
2025-07-09T16:48:10.932675Z DEBUG net: Session established remote_addr=165.154.252.73:30303 client_version=Geth/v1.15.10-stable-2bf8a789/linux-amd64/go1.24.2 peer_id=0xec0d22c39f9695b2bb884ad43d95bb750e9a5691a562c25e182963300c12baea8cd704af19302f189d89a06e07ab85238e35ff17ee14b064ab4a50cd80510c2d total_active=11 kind=outgoing peer_enode=enode://ec0d22c39f9695b2bb884ad43d95bb750e9a5691a562c25e182963300c12baea8cd704af19302f189d89a06e07ab85238e35ff17ee14b064ab4a50cd80510c2d@165.154.252.73:30303
2025-07-09T16:48:11.522772Z DEBUG net: Session established remote_addr=64.31.53.93:30303 client_version=erigon/v3.0.10-9be6eea7/linux-amd64/go1.23.10 peer_id=0x754703ee32f62f062c53ae2f34c3ed2d0009d0ac5a47e42c180229e9f94032ee561b4ab293240cf689e178e86053ff09173c3d569ed43aee021a80d1b01456d2 total_active=12 kind=outgoing peer_enode=enode://754703ee32f62f062c53ae2f34c3ed2d0009d0ac5a47e42c180229e9f94032ee561b4ab293240cf689e178e86053ff09173c3d569ed43aee021a80d1b01456d2@64.31.53.93:30303
2025-07-09T16:48:12.952056Z DEBUG engine::tree: received new engine message msg=Request(NewPayload(parent: 0xf0f16d2dd233cec1af083a84e8b5f3f0165acb3bab35dbd6be9f01c76788252e, number: 22882919, hash: 0x43b6cab4a03ece3df6836302c07d1a5ed2da3d1afc7b9da271536a48dec25145))
2025-07-09T16:48:12.953902Z DEBUG engine::tree: Inserting new block into tree block=NumHash { number: 22882919, hash: 0x43b6cab4a03ece3df6836302c07d1a5ed2da3d1afc7b9da271536a48dec25145 } parent=0xf0f16d2dd233cec1af083a84e8b5f3f0165acb3bab35dbd6be9f01c76788252e state_root=0xbf963ae7169e7382acd8e7fd74b2f2abb53a3054ac233441c5c3a2c87ddc37a6
2025-07-09T16:48:12.953912Z DEBUG reth::cli: Event: Handler(BlockReceived(NumHash { number: 22882919, hash: 0x43b6cab4a03ece3df6836302c07d1a5ed2da3d1afc7b9da271536a48dec25145 }))
2025-07-09T16:48:12.953944Z  INFO reth_node_events::node: Received block from consensus engine number=22882919 hash=0x43b6cab4a03ece3df6836302c07d1a5ed2da3d1afc7b9da271536a48dec25145
2025-07-09T16:48:12.954636Z DEBUG engine::tree: no canonical state found for block hash=0xf0f16d2dd233cec1af083a84e8b5f3f0165acb3bab35dbd6be9f01c76788252e
2025-07-09T16:48:13.083851Z DEBUG engine::tree: received new engine message msg=Request(ForkchoiceUpdated { state: ForkchoiceState { head_block_hash: 0x43b6cab4a03ece3df6836302c07d1a5ed2da3d1afc7b9da271536a48dec25145, safe_block_hash: 0xb63c5d0449df0d4bbae63007b5f516387a7b25ecbf5ab70523079dee75235d9c, finalized_block_hash: 0x74d3b19599fec1ad223f223d9f59da013097557b6b2fe90240bb499d14ff3f10 }, has_payload_attributes: false })
2025-07-09T16:48:13.083937Z DEBUG engine::tree: New head block not found in inmemory tree state new_head=0x43b6cab4a03ece3df6836302c07d1a5ed2da3d1afc7b9da271536a48dec25145
2025-07-09T16:48:13.083968Z DEBUG reth::cli: Event: Handler(ForkchoiceUpdated(ForkchoiceState { head_block_hash: 0x43b6cab4a03ece3df6836302c07d1a5ed2da3d1afc7b9da271536a48dec25145, safe_block_hash: 0xb63c5d0449df0d4bbae63007b5f516387a7b25ecbf5ab70523079dee75235d9c, finalized_block_hash: 0x74d3b19599fec1ad223f223d9f59da013097557b6b2fe90240bb499d14ff3f10 }, Syncing))
2025-07-09T16:48:13.583890Z DEBUG engine::tree: received new engine message msg=DownloadedBlocks(1 blocks)
2025-07-09T16:48:13.583924Z DEBUG engine::tree: Inserting new block into tree block=NumHash { number: 22882867, hash: 0xe556a78745383dafc40643f711f2b28abf004979740b4cb0c3bac55d15d145ba } parent=0xe8fba106128b49b30a0501fda7db424322e2557a6a38a49fbd672703d56109de state_root=0xb930ab16a9cadb4619e7a422d4f54cabfeab1a431893e1b1ff60bc9d9a8c33f3
2025-07-09T16:48:13.584560Z DEBUG engine::tree: no canonical state found for block hash=0xe8fba106128b49b30a0501fda7db424322e2557a6a38a49fbd672703d56109de
2025-07-09T16:48:22.242166Z DEBUG net::session: timed out outgoing request id=1053 remote_peer_id=0x62fab63af0b0d99f4d7aa1437e08dc947cc4a4607259f39c505d8166e41f96ef50b5e7f5b6f3522a6ef38a9dc81e92e443b438f733c3ff8853e0735221b25075
2025-07-09T16:48:22.242183Z DEBUG net::peers: applied reputation change reputation=-4096 banned=false kind=Timeout
2025-07-09T16:48:22.242191Z DEBUG ChainOrchestrator::poll: downloaders: Header range download failed err=request timed out while awaiting response this.start_hash=0xa7daf935bb83a5df0103f915eabc1b4d296789590898b67a1beb7206cd4982f3
2025-07-09T16:48:22.520478Z DEBUG engine::tree: received new engine message msg=Request(ForkchoiceUpdated { state: ForkchoiceState { head_block_hash: 0x43b6cab4a03ece3df6836302c07d1a5ed2da3d1afc7b9da271536a48dec25145, safe_block_hash: 0xcfa91f11e1237d99ea852d6120e6e43124df17987d56b309d90c7a21d286de91, finalized_block_hash: 0xb63c5d0449df0d4bbae63007b5f516387a7b25ecbf5ab70523079dee75235d9c }, has_payload_attributes: false })
2025-07-09T16:48:22.520549Z DEBUG engine::tree: New head block not found in inmemory tree state new_head=0x43b6cab4a03ece3df6836302c07d1a5ed2da3d1afc7b9da271536a48dec25145
2025-07-09T16:48:22.520564Z DEBUG reth::cli: Event: Handler(ForkchoiceUpdated(ForkchoiceState { head_block_hash: 0x43b6cab4a03ece3df6836302c07d1a5ed2da3d1afc7b9da271536a48dec25145, safe_block_hash: 0xcfa91f11e1237d99ea852d6120e6e43124df17987d56b309d90c7a21d286de91, finalized_block_hash: 0xb63c5d0449df0d4bbae63007b5f516387a7b25ecbf5ab70523079dee75235d9c }, Syncing))
2025-07-09T16:48:22.520609Z  INFO reth_node_events::node: Received forkchoice updated message when syncing head_block_hash=0x43b6cab4a03ece3df6836302c07d1a5ed2da3d1afc7b9da271536a48dec25145 safe_block_hash=0xcfa91f11e1237d99ea852d6120e6e43124df17987d56b309d90c7a21d286de91 finalized_block_hash=0xb63c5d0449df0d4bbae63007b5f516387a7b25ecbf5ab70523079dee75235d9c
2025-07-09T16:48:22.844768Z DEBUG engine::tree: received new engine message msg=DownloadedBlocks(1 blocks)
2025-07-09T16:48:22.844794Z DEBUG engine::tree: Inserting new block into tree block=NumHash { number: 22882866, hash: 0xe8fba106128b49b30a0501fda7db424322e2557a6a38a49fbd672703d56109de } parent=0x463bb0fd8ce2eddb5c1a4504afd3375acea1f6d2c9929c4160cdb3594719315e state_root=0xc3c1119208c1833bef052209dee332c09f8a9fbf8981993984b941422450027e
2025-07-09T16:48:22.845692Z DEBUG engine::tree: no canonical state found for block hash=0x463bb0fd8ce2eddb5c1a4504afd3375acea1f6d2c9929c4160cdb3594719315e
2025-07-09T16:48:22.845936Z DEBUG engine::tree: emitting backfill action event
2025-07-09T16:48:22.846046Z DEBUG engine::tree: received new engine message msg=Event(BackfillSyncStarted)
2025-07-09T16:48:22.846048Z DEBUG reth::cli: Event: BackfillSyncStarted
2025-07-09T16:48:22.846051Z DEBUG engine::tree: received backfill sync started event
2025-07-09T16:48:22.846288Z DEBUG reth_node_events::node: Static File Producer started targets=StaticFileTargets { headers: None, receipts: None, transactions: None, block_meta: Some(0..=22882824) }
2025-07-09T16:48:22.846274Z DEBUG ChainOrchestrator::poll: static_file: StaticFileProducer started targets=StaticFileTargets { headers: None, receipts: None, transactions: None, block_meta: Some(0..=22882824) }
2025-07-09T16:48:22.846311Z DEBUG ChainOrchestrator::poll: static_file: StaticFileProducer finished targets=StaticFileTargets { headers: None, receipts: None, transactions: None, block_meta: Some(0..=22882824) } elapsed=4.529µs
2025-07-09T16:48:22.846337Z DEBUG reth_node_events::node: Static File Producer finished targets=StaticFileTargets { headers: None, receipts: None, transactions: None, block_meta: Some(0..=22882824) } elapsed=4.529µs
2025-07-09T16:48:22.846458Z DEBUG ChainOrchestrator::poll: pruner: Pruner started tip_block_number=8523798
2025-07-09T16:48:22.846469Z DEBUG ChainOrchestrator::poll: pruner: Nothing to prune for the segment segment=Headers purpose=StaticFile
2025-07-09T16:48:22.846474Z DEBUG ChainOrchestrator::poll: pruner: Nothing to prune for the segment segment=Transactions purpose=StaticFile
2025-07-09T16:48:22.846488Z DEBUG ChainOrchestrator::poll: pruner: Segment pruning started segment=Receipts purpose=StaticFile to_block=0 prune_mode=Before(1)
2025-07-09T16:48:22.846864Z DEBUG ChainOrchestrator::poll: pruner: Segment pruning finished segment=Receipts purpose=StaticFile to_block=0 prune_mode=Before(1) segment_output.pruned=0
2025-07-09T16:48:22.846868Z DEBUG ChainOrchestrator::poll: pruner: Pruner finished tip_block_number=8523798 elapsed=402.696µs deleted_entries=0 limiter=PruneLimiter { deleted_entries_limit: Some(PruneDeletedEntriesLimit { limit: 18446744073709551615, deleted: 0 }), time_limit: None } output=PrunerOutput { progress: Finished, segments: [(Receipts, SegmentOutput { progress: Finished, pruned: 0, checkpoint: None })] } stats=[]
2025-07-09T16:48:22.846919Z DEBUG ChainOrchestrator::poll: storage::db::mdbx: Commit total_duration=27.113µs commit_latency=Some(CommitLatency(MDBX_commit_latency { preparation: 0, gc_wallclock: 0, audit: 0, write: 0, sync: 0, ending: 0, whole: 1, gc_cputime: 0, gc_prof: MDBX_commit_latency__bindgen_ty_1 { wloops: 0, coalescences: 0, wipes: 0, flushes: 0, kicks: 0, work_counter: 0, work_rtime_monotonic: 0, work_xtime_cpu: 0, work_rsteps: 0, work_xpages: 0, work_majflt: 0, self_counter: 0, self_rtime_monotonic: 0, self_xtime_cpu: 0, self_rsteps: 0, self_xpages: 0, self_majflt: 0, pnl_merge_work: MDBX_commit_latency__bindgen_ty_1__bindgen_ty_1 { time: 0, volume: 0, calls: 0 }, pnl_merge_self: MDBX_commit_latency__bindgen_ty_1__bindgen_ty_1 { time: 0, volume: 0, calls: 0 } } })) is_read_only=false
2025-07-09T16:48:22.847219Z  INFO reth_node_events::node: Preparing stage pipeline_stages=1/15 stage=Era checkpoint=0 target=None
2025-07-09T16:48:22.847230Z  INFO reth_node_events::node: Executing stage pipeline_stages=1/15 stage=Era checkpoint=0 target=None
2025-07-09T16:48:22.870221Z  INFO reth_node_events::node: Finished stage pipeline_stages=1/15 stage=Era checkpoint=0 target=None
2025-07-09T16:48:22.870228Z DEBUG ChainOrchestrator::poll: storage::db::mdbx: Commit total_duration=27.674µs commit_latency=Some(CommitLatency(MDBX_commit_latency { preparation: 0, gc_wallclock: 0, audit: 0, write: 0, sync: 0, ending: 0, whole: 1, gc_cputime: 0, gc_prof: MDBX_commit_latency__bindgen_ty_1 { wloops: 0, coalescences: 0, wipes: 0, flushes: 0, kicks: 0, work_counter: 0, work_rtime_monotonic: 0, work_xtime_cpu: 0, work_rsteps: 0, work_xpages: 0, work_majflt: 0, self_counter: 0, self_rtime_monotonic: 0, self_xtime_cpu: 0, self_rsteps: 0, self_xpages: 0, self_majflt: 0, pnl_merge_work: MDBX_commit_latency__bindgen_ty_1__bindgen_ty_1 { time: 0, volume: 0, calls: 0 }, pnl_merge_self: MDBX_commit_latency__bindgen_ty_1__bindgen_ty_1 { time: 0, volume: 0, calls: 0 } } })) is_read_only=false
2025-07-09T16:48:22.870264Z  INFO reth_node_events::node: Preparing stage pipeline_stages=2/15 stage=Headers checkpoint=22882824 target=0
2025-07-09T16:48:22.870275Z DEBUG ChainOrchestrator::poll: sync::stages::headers: Commencing sync tip=Hash(0xb63c5d0449df0d4bbae63007b5f516387a7b25ecbf5ab70523079dee75235d9c) head=0x74d3b19599fec1ad223f223d9f59da013097557b6b2fe90240bb499d14ff3f10
2025-07-09T16:48:23.330431Z DEBUG ChainOrchestrator::poll: sync::stages::headers: Commencing sync tip=Hash(0xb63c5d0449df0d4bbae63007b5f516387a7b25ecbf5ab70523079dee75235d9c) head=0x74d3b19599fec1ad223f223d9f59da013097557b6b2fe90240bb499d14ff3f10
2025-07-09T16:48:23.330458Z  INFO ChainOrchestrator::poll: sync::stages::headers: Received headers total=32 from_block=22882856 to_block=22882825
2025-07-09T16:48:23.330510Z  INFO ChainOrchestrator::poll: sync::stages::headers: Writing headers total=32
2025-07-09T16:48:23.330548Z  INFO reth_node_events::node: Executing stage pipeline_stages=2/15 stage=Headers checkpoint=22882824 target=0
2025-07-09T16:48:23.331230Z  INFO ChainOrchestrator::poll: sync::stages::headers: Writing headers hash index total=32
2025-07-09T16:48:23.375228Z  INFO reth_node_events::node: Finished stage pipeline_stages=2/15 stage=Headers checkpoint=22882856 target=0 stage_progress=100.00%
2025-07-09T16:48:23.382685Z DEBUG ChainOrchestrator::poll: provider::static_file: Commit segment=Headers path="/root/.local/share/reth/mainnet/static_files/static_file_headers_22500000_22999999" duration=7.48813ms
2025-07-09T16:48:23.383852Z DEBUG ChainOrchestrator::poll: storage::db::mdbx: Commit total_duration=1.067233ms commit_latency=Some(CommitLatency(MDBX_commit_latency { preparation: 0, gc_wallclock: 12, audit: 0, write: 0, sync: 54, ending: 0, whole: 67, gc_cputime: 8, gc_prof: MDBX_commit_latency__bindgen_ty_1 { wloops: 0, coalescences: 0, wipes: 0, flushes: 0, kicks: 0, work_counter: 0, work_rtime_monotonic: 0, work_xtime_cpu: 0, work_rsteps: 0, work_xpages: 0, work_majflt: 0, self_counter: 0, self_rtime_monotonic: 0, self_xtime_cpu: 0, self_rsteps: 0, self_xpages: 0, self_majflt: 0, pnl_merge_work: MDBX_commit_latency__bindgen_ty_1__bindgen_ty_1 { time: 0, volume: 0, calls: 0 }, pnl_merge_self: MDBX_commit_latency__bindgen_ty_1__bindgen_ty_1 { time: 0, volume: 0, calls: 0 } } })) is_read_only=false
2025-07-09T16:48:23.383899Z  INFO reth_node_events::node: Preparing stage pipeline_stages=3/15 stage=Bodies checkpoint=22882824 target=22882856
2025-07-09T16:48:23.383904Z  INFO downloaders::bodies: Downloading bodies count=32 range=22882825..=22882856
2025-07-09T16:48:23.680100Z  INFO reth_node_events::node: Executing stage pipeline_stages=3/15 stage=Bodies checkpoint=22882824 target=22882856
2025-07-09T16:48:23.680521Z DEBUG ChainOrchestrator::poll: sync::stages::bodies: Commencing sync stage_progress=22882825 target=22882856
2025-07-09T16:48:23.701291Z  INFO reth_node_events::node: Finished stage pipeline_stages=3/15 stage=Bodies checkpoint=22882856 target=22882856 stage_progress=100.00%
2025-07-09T16:48:23.703606Z DEBUG ChainOrchestrator::poll: provider::static_file: Commit segment=Transactions path="/root/.local/share/reth/mainnet/static_files/static_file_transactions_22500000_22999999" duration=2.377993ms
2025-07-09T16:48:23.704263Z DEBUG ChainOrchestrator::poll: storage::db::mdbx: Commit total_duration=594.08µs commit_latency=Some(CommitLatency(MDBX_commit_latency { preparation: 0, gc_wallclock: 3, audit: 0, write: 0, sync: 32, ending: 0, whole: 36, gc_cputime: 0, gc_prof: MDBX_commit_latency__bindgen_ty_1 { wloops: 0, coalescences: 0, wipes: 0, flushes: 0, kicks: 0, work_counter: 0, work_rtime_monotonic: 0, work_xtime_cpu: 0, work_rsteps: 0, work_xpages: 0, work_majflt: 0, self_counter: 0, self_rtime_monotonic: 0, self_xtime_cpu: 0, self_rsteps: 0, self_xpages: 0, self_majflt: 0, pnl_merge_work: MDBX_commit_latency__bindgen_ty_1__bindgen_ty_1 { time: 0, volume: 0, calls: 0 }, pnl_merge_self: MDBX_commit_latency__bindgen_ty_1__bindgen_ty_1 { time: 0, volume: 0, calls: 0 } } })) is_read_only=false
2025-07-09T16:48:23.704332Z  INFO reth_node_events::node: Preparing stage pipeline_stages=4/15 stage=SenderRecovery checkpoint=22882824 target=22882856
2025-07-09T16:48:23.704335Z  INFO ChainOrchestrator::poll: sync::stages::sender_recovery: Recovering senders tx_range=2883241786..2883248356
2025-07-09T16:48:23.704342Z  INFO reth_node_events::node: Executing stage pipeline_stages=4/15 stage=SenderRecovery checkpoint=22882824 target=22882856
2025-07-09T16:48:23.704390Z DEBUG ChainOrchestrator::poll: sync::stages::sender_recovery: Sending batch for processing tx_range=2883241786..2883248356
2025-07-09T16:48:23.704405Z DEBUG ChainOrchestrator::poll: sync::stages::sender_recovery: Appending recovered senders to the database tx_range=2883241786..2883248356
2025-07-09T16:48:23.727784Z DEBUG ChainOrchestrator::poll: sync::stages::sender_recovery: Finished recovering senders batch tx_range=2883241786..2883248356
2025-07-09T16:48:23.727887Z  INFO reth_node_events::node: Finished stage pipeline_stages=4/15 stage=SenderRecovery checkpoint=22882856 target=22882856 stage_progress=100.00%
2025-07-09T16:48:23.728637Z DEBUG ChainOrchestrator::poll: storage::db::mdbx: Commit total_duration=780.183µs commit_latency=Some(CommitLatency(MDBX_commit_latency { preparation: 0, gc_wallclock: 3, audit: 0, write: 0, sync: 45, ending: 0, whole: 49, gc_cputime: 0, gc_prof: MDBX_commit_latency__bindgen_ty_1 { wloops: 0, coalescences: 0, wipes: 0, flushes: 0, kicks: 0, work_counter: 0, work_rtime_monotonic: 0, work_xtime_cpu: 0, work_rsteps: 0, work_xpages: 0, work_majflt: 0, self_counter: 0, self_rtime_monotonic: 0, self_xtime_cpu: 0, self_rsteps: 0, self_xpages: 0, self_majflt: 0, pnl_merge_work: MDBX_commit_latency__bindgen_ty_1__bindgen_ty_1 { time: 0, volume: 0, calls: 0 }, pnl_merge_self: MDBX_commit_latency__bindgen_ty_1__bindgen_ty_1 { time: 0, volume: 0, calls: 0 } } })) is_read_only=false
2025-07-09T16:48:23.728706Z  INFO reth_node_events::node: Preparing stage pipeline_stages=5/15 stage=Execution checkpoint=8523798 target=22882856
2025-07-09T16:48:23.728716Z  INFO reth_node_events::node: Executing stage pipeline_stages=5/15 stage=Execution checkpoint=8523798 target=22882856
2025-07-09T16:48:23.728726Z DEBUG ChainOrchestrator::poll: sync::stages::execution: Calculating gas used from headers range=8523799..=22882856
2025-07-09T16:48:24.992899Z DEBUG net: Session established remote_addr=213.250.40.250:30307 client_version=Geth/v1.15.11-stable-36b2371c/linux-amd64/go1.24.2 peer_id=0x7e31faf60cdaa672606846ebfa860c5732a9537d6e482db8c8af89235386e931c76ea25b73023f9c5bbfa95ed0c8c1ff98cee484cc188a306c5ae1f31d405078 total_active=13 kind=outgoing peer_enode=enode://7e31faf60cdaa672606846ebfa860c5732a9537d6e482db8c8af89235386e931c76ea25b73023f9c5bbfa95ed0c8c1ff98cee484cc188a306c5ae1f31d405078@213.250.40.250:30307
2025-07-09T16:48:25.729849Z DEBUG engine::tree: received new engine message msg=Request(NewPayload(parent: 0x43b6cab4a03ece3df6836302c07d1a5ed2da3d1afc7b9da271536a48dec25145, number: 22882920, hash: 0x9c32fab17a8167cece448ac86d80032c773db3a2f19e018690338004ffa7ed32))
2025-07-09T16:48:25.731696Z DEBUG reth::cli: Event: Handler(BlockReceived(NumHash { number: 22882920, hash: 0x9c32fab17a8167cece448ac86d80032c773db3a2f19e018690338004ffa7ed32 }))
2025-07-09T16:48:25.731743Z  INFO reth_node_events::node: Received block from consensus engine number=22882920 hash=0x9c32fab17a8167cece448ac86d80032c773db3a2f19e018690338004ffa7ed32
2025-07-09T16:48:27.753792Z DEBUG net: Session established remote_addr=37.27.237.223:30305 client_version=reth/v1.5.0-61e38f9/x86_64-unknown-linux-gnu peer_id=0xc3c88c83f428940db0c703816da1238f44c1bfbdc6c1333d2e5231f77025350274df003f5e08f6e0b5f54530e9f82dcb5f7eb986615c8bea34fe7a1c4a74445c total_active=14 kind=outgoing peer_enode=enode://c3c88c83f428940db0c703816da1238f44c1bfbdc6c1333d2e5231f77025350274df003f5e08f6e0b5f54530e9f82dcb5f7eb986615c8bea34fe7a1c4a74445c@37.27.237.223:30305
2025-07-09T16:48:27.753862Z DEBUG net::session: failed to receive message err=disconnected remote_peer_id=0xc3c88c83f428940db0c703816da1238f44c1bfbdc6c1333d2e5231f77025350274df003f5e08f6e0b5f54530e9f82dcb5f7eb986615c8bea34fe7a1c4a74445c
2025-07-09T16:48:28.152408Z DEBUG ChainOrchestrator::poll: sync::stages::execution: Finished calculating gas used from headers range=8523799..=22882856 duration=4.423670926s
2025-07-09T16:48:28.152429Z DEBUG ChainOrchestrator::poll: sync::stages::execution: Executing range start=8523799 end=22882856
2025-07-09T16:48:29.090299Z  INFO reth::cli: Status connected_peers=13 stage=Execution checkpoint=8523798 target=22882856
2025-07-09T16:48:29.333558Z DEBUG engine::tree: received new engine message msg=Request(ForkchoiceUpdated { state: ForkchoiceState { head_block_hash: 0x9c32fab17a8167cece448ac86d80032c773db3a2f19e018690338004ffa7ed32, safe_block_hash: 0xcfa91f11e1237d99ea852d6120e6e43124df17987d56b309d90c7a21d286de91, finalized_block_hash: 0xb63c5d0449df0d4bbae63007b5f516387a7b25ecbf5ab70523079dee75235d9c }, has_payload_attributes: false })
2025-07-09T16:48:29.333622Z DEBUG reth::cli: Event: Handler(ForkchoiceUpdated(ForkchoiceState { head_block_hash: 0x9c32fab17a8167cece448ac86d80032c773db3a2f19e018690338004ffa7ed32, safe_block_hash: 0xcfa91f11e1237d99ea852d6120e6e43124df17987d56b309d90c7a21d286de91, finalized_block_hash: 0xb63c5d0449df0d4bbae63007b5f516387a7b25ecbf5ab70523079dee75235d9c }, Syncing))
2025-07-09T16:48:34.110688Z DEBUG net: Session established remote_addr=136.33.201.13:30403 client_version=Geth/v1.15.11-stable-36b2371c/linux-amd64/go1.24.2 peer_id=0xc0eed204c662788328a51af111d96e19a7499bd75c8ac59c442f91de7449ab0e5eba9c26730b72406aac76652cbf253475a6886c1edc27aa641f7c0c79aaa337 total_active=14 kind=outgoing peer_enode=enode://c0eed204c662788328a51af111d96e19a7499bd75c8ac59c442f91de7449ab0e5eba9c26730b72406aac76652cbf253475a6886c1edc27aa641f7c0c79aaa337@136.33.201.13:30403
2025-07-09T16:48:37.924109Z DEBUG engine::tree: received new engine message msg=Request(NewPayload(parent: 0x9c32fab17a8167cece448ac86d80032c773db3a2f19e018690338004ffa7ed32, number: 22882921, hash: 0x0e4f65d24fae3d4c5347953452a26ed9ce6234df9475677823acf8a423141a1f))
2025-07-09T16:48:37.925889Z DEBUG reth::cli: Event: Handler(BlockReceived(NumHash { number: 22882921, hash: 0x0e4f65d24fae3d4c5347953452a26ed9ce6234df9475677823acf8a423141a1f }))
2025-07-09T16:48:37.925919Z  INFO reth_node_events::node: Received block from consensus engine number=22882921 hash=0x0e4f65d24fae3d4c5347953452a26ed9ce6234df9475677823acf8a423141a1f
2025-07-09T16:48:38.065701Z DEBUG engine::tree: received new engine message msg=Request(ForkchoiceUpdated { state: ForkchoiceState { head_block_hash: 0x0e4f65d24fae3d4c5347953452a26ed9ce6234df9475677823acf8a423141a1f, safe_block_hash: 0xcfa91f11e1237d99ea852d6120e6e43124df17987d56b309d90c7a21d286de91, finalized_block_hash: 0xb63c5d0449df0d4bbae63007b5f516387a7b25ecbf5ab70523079dee75235d9c }, has_payload_attributes: false })
2025-07-09T16:48:38.065771Z DEBUG reth::cli: Event: Handler(ForkchoiceUpdated(ForkchoiceState { head_block_hash: 0x0e4f65d24fae3d4c5347953452a26ed9ce6234df9475677823acf8a423141a1f, safe_block_hash: 0xcfa91f11e1237d99ea852d6120e6e43124df17987d56b309d90c7a21d286de91, finalized_block_hash: 0xb63c5d0449df0d4bbae63007b5f516387a7b25ecbf5ab70523079dee75235d9c }, Syncing))
2025-07-09T16:48:38.225547Z  INFO ChainOrchestrator::poll: sync::stages::execution: Executed block range start=8523799 end=8523878 throughput="58.21 Mgas/second"
2025-07-09T16:48:39.905477Z DEBUG net: Session established remote_addr=40.160.13.217:30303 client_version=r peer_id=0x3db469f727c9cdca42d0b63922caabd21c683735629d053f0b16cd4d9ae4d6887e7ea2cb4b18b6238479c13d21998ae0e86487eed5a7ce2e30648c6e6aa03547 total_active=15 kind=outgoing peer_enode=enode://3db469f727c9cdca42d0b63922caabd21c683735629d053f0b16cd4d9ae4d6887e7ea2cb4b18b6238479c13d21998ae0e86487eed5a7ce2e30648c6e6aa03547@40.160.13.217:30303
2025-07-09T16:48:39.905612Z DEBUG net::session: failed to receive message err=disconnected remote_peer_id=0x3db469f727c9cdca42d0b63922caabd21c683735629d053f0b16cd4d9ae4d6887e7ea2cb4b18b6238479c13d21998ae0e86487eed5a7ce2e30648c6e6aa03547
2025-07-09T16:48:41.662434Z DEBUG net: Session established remote_addr=65.109.60.60:30303 client_version=Geth/v1.15.11-stable-36b2371c/linux-amd64/go1.24.2 peer_id=0x51479d80bc2997a1371af658fb1b20a6c4dba0c74487e10288d9cc5a185791cc8f8de2962f8b75013fd9603304a68c1ddeb5436afe8ff01fab2b4f32f44973bb total_active=15 kind=outgoing peer_enode=enode://51479d80bc2997a1371af658fb1b20a6c4dba0c74487e10288d9cc5a185791cc8f8de2962f8b75013fd9603304a68c1ddeb5436afe8ff01fab2b4f32f44973bb@65.109.60.60:30303
2025-07-09T16:48:48.228333Z  INFO ChainOrchestrator::poll: sync::stages::execution: Executed block range start=8523879 end=8524004 throughput="97.41 Mgas/second"
2025-07-09T16:48:49.006967Z DEBUG net: Session established remote_addr=47.91.65.144:30303 client_version=Geth/v1.15.10-stable-2bf8a789/linux-amd64/go1.24.2 peer_id=0x523199fd2359da67a97cdcf5688abd0e7acb584f14af2dfa6425373e079e066a9272cdf46b75f29a3facc75b8275f9af8ce4b8e5d27bb1cb342a2e89fe48ab27 total_active=16 kind=outgoing peer_enode=enode://523199fd2359da67a97cdcf5688abd0e7acb584f14af2dfa6425373e079e066a9272cdf46b75f29a3facc75b8275f9af8ce4b8e5d27bb1cb342a2e89fe48ab27@47.91.65.144:30303
2025-07-09T16:48:50.122383Z DEBUG engine::tree: received new engine message msg=Request(NewPayload(parent: 0x0e4f65d24fae3d4c5347953452a26ed9ce6234df9475677823acf8a423141a1f, number: 22882922, hash: 0x0e6ff6f6377818ab972be161a604856d273cd2682b5eff7353d7ea9456770c36))
2025-07-09T16:48:50.123961Z DEBUG reth::cli: Event: Handler(BlockReceived(NumHash { number: 22882922, hash: 0x0e6ff6f6377818ab972be161a604856d273cd2682b5eff7353d7ea9456770c36 }))
2025-07-09T16:48:50.124025Z  INFO reth_node_events::node: Received block from consensus engine number=22882922 hash=0x0e6ff6f6377818ab972be161a604856d273cd2682b5eff7353d7ea9456770c36
2025-07-09T16:48:50.667264Z DEBUG engine::tree: received new engine message msg=Request(ForkchoiceUpdated { state: ForkchoiceState { head_block_hash: 0x0e6ff6f6377818ab972be161a604856d273cd2682b5eff7353d7ea9456770c36, safe_block_hash: 0xcfa91f11e1237d99ea852d6120e6e43124df17987d56b309d90c7a21d286de91, finalized_block_hash: 0xb63c5d0449df0d4bbae63007b5f516387a7b25ecbf5ab70523079dee75235d9c }, has_payload_attributes: false })
2025-07-09T16:48:50.667336Z DEBUG reth::cli: Event: Handler(ForkchoiceUpdated(ForkchoiceState { head_block_hash: 0x0e6ff6f6377818ab972be161a604856d273cd2682b5eff7353d7ea9456770c36, safe_block_hash: 0xcfa91f11e1237d99ea852d6120e6e43124df17987d56b309d90c7a21d286de91, finalized_block_hash: 0xb63c5d0449df0d4bbae63007b5f516387a7b25ecbf5ab70523079dee75235d9c }, Syncing))
2025-07-09T16:48:52.628147Z DEBUG net: Session established remote_addr=40.160.16.181:30303 client_version=r peer_id=0xb6083ecc457da689ece677a3d66380f6db378c22429a7bcb231519ba388f2fb3b90244c39783ce0a358cb6286db1a8093428313540b3d20d928f67d4bfb25c05 total_active=17 kind=outgoing peer_enode=enode://b6083ecc457da689ece677a3d66380f6db378c22429a7bcb231519ba388f2fb3b90244c39783ce0a358cb6286db1a8093428313540b3d20d928f67d4bfb25c05@40.160.16.181:30303
2025-07-09T16:48:52.628266Z DEBUG net::session: failed to receive message err=disconnected remote_peer_id=0xb6083ecc457da689ece677a3d66380f6db378c22429a7bcb231519ba388f2fb3b90244c39783ce0a358cb6286db1a8093428313540b3d20d928f67d4bfb25c05
2025-07-09T16:48:54.090587Z  INFO reth::cli: Status connected_peers=16 stage=Execution checkpoint=8523798 target=22882856
2025-07-09T16:48:55.171794Z DEBUG net: Session established remote_addr=57.128.187.17:37307 client_version=Geth/v1.15.11-stable-36b2371c/linux-amd64/go1.24.2 peer_id=0x10bc996d9be21e07b5b6070719121f123df83d88f87c2517445dfbcfd79c1ef6248c152c898a6b456ab893582fd0c601642a7c4a0ffe4c39de7c4ece90683d68 total_active=17 kind=outgoing peer_enode=enode://10bc996d9be21e07b5b6070719121f123df83d88f87c2517445dfbcfd79c1ef6248c152c898a6b456ab893582fd0c601642a7c4a0ffe4c39de7c4ece90683d68@57.128.187.17:37307
2025-07-09T16:48:56.547647Z DEBUG net: Session established remote_addr=132.145.203.59:30303 client_version=Geth/v1.15.11-stable-36b2371c/linux-amd64/go1.24.2 peer_id=0xe709bb520063b737e50cfeec20b5782ab790535fee091087fef27669242837948fa95835cdc6a7bfd8d57b6a584bd6ba5a74a60445bd88ba41d001d56dc33398 total_active=18 kind=outgoing peer_enode=enode://e709bb520063b737e50cfeec20b5782ab790535fee091087fef27669242837948fa95835cdc6a7bfd8d57b6a584bd6ba5a74a60445bd88ba41d001d56dc33398@132.145.203.59:30303
2025-07-09T16:48:58.259620Z  INFO ChainOrchestrator::poll: sync::stages::execution: Executed block range start=8524005 end=8524200 throughput="145.77 Mgas/second"
2025-07-09T16:49:01.178704Z DEBUG engine::tree: received new engine message msg=Request(NewPayload(parent: 0x0e6ff6f6377818ab972be161a604856d273cd2682b5eff7353d7ea9456770c36, number: 22882923, hash: 0xdafaa74bdabd3a8e29f78893fcb6003c781e9d13a7e4d32f8e7ad899de8251af))
2025-07-09T16:49:01.179841Z DEBUG reth::cli: Event: Handler(BlockReceived(NumHash { number: 22882923, hash: 0xdafaa74bdabd3a8e29f78893fcb6003c781e9d13a7e4d32f8e7ad899de8251af }))
2025-07-09T16:49:01.179880Z  INFO reth_node_events::node: Received block from consensus engine number=22882923 hash=0xdafaa74bdabd3a8e29f78893fcb6003c781e9d13a7e4d32f8e7ad899de8251af
2025-07-09T16:49:01.869295Z DEBUG engine::tree: received new engine message msg=Request(ForkchoiceUpdated { state: ForkchoiceState { head_block_hash: 0xdafaa74bdabd3a8e29f78893fcb6003c781e9d13a7e4d32f8e7ad899de8251af, safe_block_hash: 0xcfa91f11e1237d99ea852d6120e6e43124df17987d56b309d90c7a21d286de91, finalized_block_hash: 0xb63c5d0449df0d4bbae63007b5f516387a7b25ecbf5ab70523079dee75235d9c }, has_payload_attributes: false })
2025-07-09T16:49:01.869385Z DEBUG reth::cli: Event: Handler(ForkchoiceUpdated(ForkchoiceState { head_block_hash: 0xdafaa74bdabd3a8e29f78893fcb6003c781e9d13a7e4d32f8e7ad899de8251af, safe_block_hash: 0xcfa91f11e1237d99ea852d6120e6e43124df17987d56b309d90c7a21d286de91, finalized_block_hash: 0xb63c5d0449df0d4bbae63007b5f516387a7b25ecbf5ab70523079dee75235d9c }, Syncing))
2025-07-09T16:49:08.279534Z  INFO ChainOrchestrator::poll: sync::stages::execution: Executed block range start=8524201 end=8524431 throughput="180.98 Mgas/second"
2025-07-09T16:49:10.006146Z DEBUG net: Session established remote_addr=37.27.237.223:30305 client_version=reth/v1.5.0-61e38f9/x86_64-unknown-linux-gnu peer_id=0xc3c88c83f428940db0c703816da1238f44c1bfbdc6c1333d2e5231f77025350274df003f5e08f6e0b5f54530e9f82dcb5f7eb986615c8bea34fe7a1c4a74445c total_active=19 kind=outgoing peer_enode=enode://c3c88c83f428940db0c703816da1238f44c1bfbdc6c1333d2e5231f77025350274df003f5e08f6e0b5f54530e9f82dcb5f7eb986615c8bea34fe7a1c4a74445c@37.27.237.223:30305
2025-07-09T16:49:10.006213Z DEBUG net::session: failed to receive message err=disconnected remote_peer_id=0xc3c88c83f428940db0c703816da1238f44c1bfbdc6c1333d2e5231f77025350274df003f5e08f6e0b5f54530e9f82dcb5f7eb986615c8bea34fe7a1c4a74445c
2025-07-09T16:49:12.978952Z DEBUG engine::tree: received new engine message msg=Request(NewPayload(parent: 0xdafaa74bdabd3a8e29f78893fcb6003c781e9d13a7e4d32f8e7ad899de8251af, number: 22882924, hash: 0x7eece5fc8a8014184e5a63f0d50a8bc5f541ad4d47d0faa276687e2f8f2fa934))
2025-07-09T16:49:12.980636Z DEBUG reth::cli: Event: Handler(BlockReceived(NumHash { number: 22882924, hash: 0x7eece5fc8a8014184e5a63f0d50a8bc5f541ad4d47d0faa276687e2f8f2fa934 }))
2025-07-09T16:49:12.980676Z  INFO reth_node_events::node: Received block from consensus engine number=22882924 hash=0x7eece5fc8a8014184e5a63f0d50a8bc5f541ad4d47d0faa276687e2f8f2fa934
2025-07-09T16:49:13.422099Z DEBUG engine::tree: received new engine message msg=Request(ForkchoiceUpdated { state: ForkchoiceState { head_block_hash: 0x7eece5fc8a8014184e5a63f0d50a8bc5f541ad4d47d0faa276687e2f8f2fa934, safe_block_hash: 0xcfa91f11e1237d99ea852d6120e6e43124df17987d56b309d90c7a21d286de91, finalized_block_hash: 0xb63c5d0449df0d4bbae63007b5f516387a7b25ecbf5ab70523079dee75235d9c }, has_payload_attributes: false })
2025-07-09T16:49:13.422144Z DEBUG reth::cli: Event: Handler(ForkchoiceUpdated(ForkchoiceState { head_block_hash: 0x7eece5fc8a8014184e5a63f0d50a8bc5f541ad4d47d0faa276687e2f8f2fa934, safe_block_hash: 0xcfa91f11e1237d99ea852d6120e6e43124df17987d56b309d90c7a21d286de91, finalized_block_hash: 0xb63c5d0449df0d4bbae63007b5f516387a7b25ecbf5ab70523079dee75235d9c }, Syncing))
2025-07-09T16:49:14.080685Z DEBUG net: Session established remote_addr=136.243.38.88:30303 client_version=Geth/v1.15.10-stable-2bf8a789/linux-amd64/go1.24.2 peer_id=0x52652f4c009f9bbbed607b9e55803266ea72888ec85ad10554adc2092978c8729dc89fd771a32fba33d7e3a4033cab375b218e993e05e093b61aea43811ce906 total_active=19 kind=outgoing peer_enode=enode://52652f4c009f9bbbed607b9e55803266ea72888ec85ad10554adc2092978c8729dc89fd771a32fba33d7e3a4033cab375b218e993e05e093b61aea43811ce906@136.243.38.88:30303
2025-07-09T16:49:14.219049Z DEBUG net: Session established remote_addr=194.233.79.81:30303 client_version=reth/v1.5.0-61e38f9/x86_64-unknown-linux-gnu peer_id=0x73d0af6fccef926edf6dc7c50f4500e6ddaa3d88ac5cd55afa6e5ce192e50394b23ccc8e19590ff7f911f24f5c94d486c405856e8ed4e6a1a2fca6550c9777b8 total_active=20 kind=outgoing peer_enode=enode://73d0af6fccef926edf6dc7c50f4500e6ddaa3d88ac5cd55afa6e5ce192e50394b23ccc8e19590ff7f911f24f5c94d486c405856e8ed4e6a1a2fca6550c9777b8@194.233.79.81:30303
2025-07-09T16:49:14.219111Z DEBUG net::session: failed to receive message err=disconnected remote_peer_id=0x73d0af6fccef926edf6dc7c50f4500e6ddaa3d88ac5cd55afa6e5ce192e50394b23ccc8e19590ff7f911f24f5c94d486c405856e8ed4e6a1a2fca6550c9777b8
2025-07-09T16:49:15.448272Z ERROR ChainOrchestrator::poll: sync::pipeline: Stage encountered a validation error: block gas used mismatch: got 3407989, expected 7999018; gas spent by each transaction: [(0, 21000), (1, 59360), (2, 625713), (3, 662919), (4, 2014454), (5, 3365989), (6, 3386989), (7, 3407989)] stage=Execution bad_block=8524574
2025-07-09T16:49:15.448503Z DEBUG ChainOrchestrator::poll: storage::db::mdbx: Commit total_duration=15.681µs commit_latency=Some(CommitLatency(MDBX_commit_latency { preparation: 0, gc_wallclock: 0, audit: 0, write: 0, sync: 0, ending: 0, whole: 1, gc_cputime: 0, gc_prof: MDBX_commit_latency__bindgen_ty_1 { wloops: 0, coalescences: 0, wipes: 0, flushes: 0, kicks: 0, work_counter: 0, work_rtime_monotonic: 0, work_xtime_cpu: 0, work_rsteps: 0, work_xpages: 0, work_majflt: 0, self_counter: 0, self_rtime_monotonic: 0, self_xtime_cpu: 0, self_rsteps: 0, self_xpages: 0, self_majflt: 0, pnl_merge_work: MDBX_commit_latency__bindgen_ty_1__bindgen_ty_1 { time: 0, volume: 0, calls: 0 }, pnl_merge_self: MDBX_commit_latency__bindgen_ty_1__bindgen_ty_1 { time: 0, volume: 0, calls: 0 } } })) is_read_only=false
2025-07-09T16:49:15.448572Z ERROR ChainOrchestrator::poll: reth_engine_tree::chain: backfill sync failed err=Cannot unwind to block 8523798 as it is beyond the AccountHistory limit. Latest block: 22882856, History limit: 10064
2025-07-09T16:49:15.448581Z DEBUG reth::cli: Event: FatalError
2025-07-09T16:49:15.448586Z ERROR reth::cli: Fatal error in consensus engine
2025-07-09T16:49:15.448657Z ERROR reth::cli: shutting down due to error
2025-07-09T16:49:16.421437Z  INFO reth::cli: Initialized tracing, debug log directory: /root/.cache/reth/logs/mainnet
2025-07-09T16:49:16.422141Z  INFO reth::cli: Starting reth version="1.5.1 (dbe7ee9)"
2025-07-09T16:49:16.422157Z  INFO reth::cli: Opening database path="/root/.local/share/reth/mainnet/db"
2025-07-09T16:49:16.440895Z  INFO reth::cli: Launching node
2025-07-09T16:49:16.441017Z DEBUG reth_node_builder::launch::common: Raised file descriptor limit from=1048576 to=1048576
2025-07-09T16:49:16.442917Z  INFO reth::cli: Configuration loaded path="/root/.local/share/reth/mainnet/reth.toml"
2025-07-09T16:49:16.453180Z  INFO reth::cli: Verifying storage consistency.
2025-07-09T16:49:16.467206Z  INFO reth::cli: Database opened
2025-07-09T16:49:16.467219Z DEBUG reth::cli: Initializing genesis chain=mainnet genesis=0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3
2025-07-09T16:49:16.467273Z DEBUG reth_db_common::init: Genesis already written, skipping.
2025-07-09T16:49:16.467291Z  INFO reth::cli: 
Pre-merge hard forks (block based):
- Frontier                         @0
- Homestead                        @1150000
- Dao                              @1920000
- Tangerine                        @2463000
- SpuriousDragon                   @2675000
- Byzantium                        @4370000
- Constantinople                   @7280000
- Petersburg                       @7280000
- Istanbul                         @9069000
- MuirGlacier                      @9200000
- Berlin                           @12244000
- London                           @12965000
- ArrowGlacier                     @13773000
- GrayGlacier                      @15050000
Merge hard forks:
- Paris                            @58750000000000000000000 (network is known to be merged)
Post-merge hard forks (timestamp based):
- Shanghai                         @1681338455
- Cancun                           @1710338135
- Prague                           @1746612311
2025-07-09T16:49:16.467298Z DEBUG reth::cli: Spawning stages metrics listener task
2025-07-09T16:49:16.467388Z DEBUG reth::cli: creating components
2025-07-09T16:49:16.467422Z DEBUG txpool::blob: Removed blob store directory blob_dir="/root/.local/share/reth/mainnet/blobstore"
2025-07-09T16:49:16.467429Z DEBUG txpool::blob: Creating blob store blob_dir="/root/.local/share/reth/mainnet/blobstore"
2025-07-09T16:49:16.467772Z  INFO reth::cli: Transaction pool initialized
2025-07-09T16:49:16.467775Z DEBUG reth::cli: Spawned txpool maintenance task
2025-07-09T16:49:16.468430Z DEBUG discv4: pinging boot node record=NodeRecord { address: 157.90.35.166, udp_port: 30303, tcp_port: 30303, id: 0x4aeb4ab6c14b23e2c4cfdce879c04b0748a20d8e9b59e25ded2a08143e265c6c25936e74cbc8e641e3312ca288673d91f2f93f8e277de3cfa444ecdaaf982052 }
2025-07-09T16:49:16.468536Z DEBUG discv4: pinging boot node record=NodeRecord { address: 18.138.108.67, udp_port: 30303, tcp_port: 30303, id: 0xd860a01f9722d78051619d1e2351aba3f43f943f6f00718d1b9baa4101932a1f5011f16bb2b1bb35db20d6fe28fa0bf09636d26a87d31de9ec6203eeedb1f666 }
2025-07-09T16:49:16.468577Z DEBUG discv4: pinging boot node record=NodeRecord { address: 3.209.45.79, udp_port: 30303, tcp_port: 30303, id: 0x22a8232c3abc76a16ae9d6c3b164f98775fe226f0917b0ca871128a74a8e9630b458460865bab457221f1d448dd9791d24c4e5d88786180ac185df813a68d4de }
2025-07-09T16:49:16.468616Z DEBUG discv4: pinging boot node record=NodeRecord { address: 65.108.70.101, udp_port: 30303, tcp_port: 30303, id: 0x2b252ab6a1d0f971d9722cb839a42cb81db019ba44c08754628ab4a823487071b5695317c8ccd085219c3a03af063495b2f1da8d18218da2d6a82981b45e6ffc }
2025-07-09T16:49:16.468756Z  INFO reth::cli: P2P networking initialized enode=enode://b7280f41c110e41aff7a9cc740365f30af91429299a45431e0dc098c083cfeab7cfb2257c110da8e092850ca82e0cb66c18297e4f8c575b1acacf0e0b97bd0fe@0.0.0.0:30303
2025-07-09T16:49:16.468771Z DEBUG reth::cli: calling on_component_initialized hook
2025-07-09T16:49:16.468769Z DEBUG txpool: removing stale transactions count=0
2025-07-09T16:49:16.468911Z  INFO reth::cli: StaticFileProducer initialized
2025-07-09T16:49:16.469135Z DEBUG static_file: StaticFileProducer started targets=StaticFileTargets { headers: None, receipts: None, transactions: None, block_meta: Some(0..=22882856) }
2025-07-09T16:49:16.469147Z DEBUG static_file: StaticFileProducer finished targets=StaticFileTargets { headers: None, receipts: None, transactions: None, block_meta: Some(0..=22882856) } elapsed=4.058µs
2025-07-09T16:49:16.469143Z DEBUG discv4: pinging boot node record=NodeRecord { address: 157.90.35.166, udp_port: 30303, tcp_port: 30303, id: 0x4aeb4ab6c14b23e2c4cfdce879c04b0748a20d8e9b59e25ded2a08143e265c6c25936e74cbc8e641e3312ca288673d91f2f93f8e277de3cfa444ecdaaf982052 }
2025-07-09T16:49:16.469185Z DEBUG discv4: pinging boot node record=NodeRecord { address: 18.138.108.67, udp_port: 30303, tcp_port: 30303, id: 0xd860a01f9722d78051619d1e2351aba3f43f943f6f00718d1b9baa4101932a1f5011f16bb2b1bb35db20d6fe28fa0bf09636d26a87d31de9ec6203eeedb1f666 }
```

### Platform(s)

Linux (x86)

### Container Type

Docker

### What version/commit are you on?

reth-ethereum-cli Version: 1.5.1
Commit SHA: dbe7ee9c21392f360ff01f6307480f5d7dd73a3a
Build Timestamp: 2025-07-08T13:25:43.023391546Z
Build Features: asm_keccak,jemalloc
Build Profile: maxperf

### What database version are you on?

# reth db version
2025-07-09T16:52:12.794396Z  INFO Initialized tracing, debug log directory: /root/.cache/reth/logs/mainnet
Current database version: 2
Local database version: 2

### Which chain / network are you on?

`mainnet`

### What type of node are you running?

Full via --full flag

### What prune config do you use, if any?

_No response_

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [x] I agree to follow the Code of Conduct
