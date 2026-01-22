---
title: OOM on 1.9.3 (27a8c0f) Post Fusaka
labels:
    - C-bug
    - S-needs-triage
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.010209Z
info:
    author: keccakk
    created_at: 2025-12-03T23:07:55Z
    updated_at: 2026-01-17T02:15:52Z
---

### Describe the bug

Had an OOM post Fusaka, memory seemed to spike up suddenly after a bad block. Reth restarted after about 4 minutes later.

### Steps to reproduce

```
[Service]
User=reth
Group=reth
Type=simple
Restart=always
RestartSec=5
ExecStart=reth node \
  --datadir /xxxxx/mainnet/reth \
  --authrpc.port 8551 \
  --authrpc.jwtsecret /xxxxx/ \
  --log.file.directory /xxxxx/mainnet/reth \
  --discovery.port 30303 \
  --port 30303 \
  --metrics 127.0.0.1:9001 \
  --http \
  --http.addr 0.0.0.0 \
  --http.port 8545 \
  --http.api "eth,net,trace,txpool,web3,rpc" \
  --http.corsdomain "XXXXX,XXXXX"
```

### Node logs

```text
Dec 03 17:22:14 arch reth[2585726]: 2025-12-03T22:22:14.750100Z  INFO Block added to canonical chain number=23935838 hash=0x6b5bfe7e65085295b7a1b74ae61cda2fa2d27646ea7519df81bf038106e3bdbb peers=131 txs=233 gas_used=30.47Mgas gas_throughput=58.64Mgas/second gas_limit=60.00Mgas full=50.8% base_fee=0.02Gwei blobs=7 excess_blobs=250 elapsed=519.643546ms
Dec 03 17:22:14 arch reth[2585726]: 2025-12-03T22:22:14.919121Z  INFO Canonical chain committed number=23935838 hash=0x6b5bfe7e65085295b7a1b74ae61cda2fa2d27646ea7519df81bf038106e3bdbb elapsed=806.2µs
Dec 03 17:22:23 arch reth[2585726]: 2025-12-03T22:22:23.740790Z  INFO Status connected_peers=131 latest_block=23935838
Dec 03 17:22:25 arch reth[2585726]: 2025-12-03T22:22:25.457464Z  INFO Received block from consensus engine number=23935839 hash=0x3b28163fd808f8febb46b220b7bf588e0ac1039f22ac81a7c3c017f622bd15b8
Dec 03 17:22:25 arch reth[2585726]: 2025-12-03T22:22:25.533942Z  WARN Failed to validate header 0x3b28163fd808f8febb46b220b7bf588e0ac1039f22ac81a7c3c017f622bd15b8 against parent: invalid excess blob gas: got 32942754, expected 33117516; parent excess blob gas: 32811682, parent blob gas used: 917504 block=RecoveredBlock { block: SealedBlock { header: SealedHeader { hash: OnceLock(0x3b28163fd808f8febb46b220b7bf588e0>
Dec 03 17:22:25 arch reth[2585726]: 6cf091, 0x960b7ebf0bb53c7aa742986d3144143a99b547c9075f1afccfd7bc6e326cf092, 0xbfd2cfff8855c931101af72e6b5adcc13b26b884b98f7e1665e8afb1935164a1, 0xe66f835ef1511d29bcdd6fc432fb7aeefe364f1708b5cb78bb1dd4d25e4833d4] }]), input: 0x12aa3caf0000000000000000000000000004d60f031e000107004585fe77225b41b697c938b018e2ac67ac5a20c00044128acb08000000000000000000000000d36f184111225d065884e129566>
Dec 03 17:22:25 arch reth[2585726]: 0000000000000000000000000000000000600000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000003a225ff5f420faf56e3b9d6e0900000000000000015455c918e405a2831fbff8595c0aae35ee3db9d100000000000000000000000000000000000000000000000000000000000000800000000000000000000000006>
Dec 03 17:22:25 arch reth[2585726]: 000000000000000000000000000000000000000000000000000000000000000000002887b22536f75726365223a226c692e6669222c22416d6f756e74496e555344223a2232342e373635313339353331343732333534222c22416d6f756e744f7574555344223a2232342e393030353233393636393439383837222c22526566657272616c223a22222c22466c616773223a302c22416d6f756e744f7574223a2231313531313536363532333533373330303230383236222c2254696d65>
Dec 03 17:22:25 arch reth[2585726]: 7563926639922241532429819911437201, s: 56465499571647771781405502136090427531776215388036603515599035950417423856042 }, hash: OnceLock(<uninit>) }), Eip2930(Signed { tx: TxEip2930 { chain_id: 1, nonce: 4324266, gas_price: 66695960, gas_limit: 500000, to: Call(0xb1f89123c8e5bd15b2aac12f55d82cbf38345974), value: 12256580000000000, access_list: AccessList([]), input: 0x }, signatur>
Dec 03 17:22:25 arch reth[2585726]: 36 }, hash: OnceLock(<uninit>) }), Eip2930(Signed { tx: TxEip2930 { chain_id: 1, nonce: 4324361, gas_price: 59826855, gas_limit: 500000, to: Call(0xc4f1352ac210b8551a14c9c4c173460943c2f73f), value: 10991960000000000, access_list: AccessList([]), input: 0x }, signature: Signature { y_parity: true, r: 90003898909402786077084897247764535769234250039946682817757271321759316710652, s>
Dec 03 17:22:25 arch reth[2585726]: 7b6cbdc63d2fb801e9f61f42c287d05e9a032e34c000000000000000000000000000000000054454d5030353030312d3500000000015957759a47f657c3b8f0b93cdbe75ad6140b7042d47f80260f0523a5ed22a5000000000000000000000000000000000054454d5030353030322d350000000001d3baaa1f6478cee050d24e15b0589b565ab70c4ad6bda5d49d1da58da01017000000000000000000000000000000000054454d5030353030332d350000000004ca6649cbca137cea07>
Dec 03 17:22:25 arch reth[2585726]: 000000000000000000000000000000000008fe8a40000000000000000000000000000000000415242322d3800000000000000000000000000000000000000000000000000000000000000000000000000008fe8a4000000000000000000000000000000000041544f4d2d380000000000000000000000000000000000000000000000000000000000000000000000000006106b22000000000000000000000000000000000041544f4d322d38000000000000000000000000000000000000>
Dec 03 17:22:25 arch reth[2585726]: 0000000000054454d5030383031352d3800000000000000000000000000000000000000000000000000000000800000000010f852000000000000000000000000000000000054454d5030383031362d3800000000000000000000000000000000000000000000000000000000800000000010f852000000000000000000000000000000000054454d5030383031372d3800000000000000000000000000000000000000000000000000000000800000000010f85200000000000000000000>
Dec 03 17:22:25 arch reth[2585726]: 000000009742dd76d59d0126ab50000114bfbe1331b6e81 }, signature: Signature { y_parity: true, r: 11154418555959077016783757278161942749407944354919476574993022655799437683848, s: 48456368501396695840195591318452331796222797374434315361734623674686341430281 }, hash: OnceLock(<uninit>) }), Eip1559(Signed { tx: TxEip1559 { chain_id: 1, nonce: 94921, gas_limit: 80803, max_fee_per_gas: 2>
Dec 03 17:22:25 arch reth[2585726]: Eip1559(Signed { tx: TxEip1559 { chain_id: 1, nonce: 495, gas_limit: 190616, max_fee_per_gas: 27701337, max_priority_fee_per_gas: 27701337, to: Call(0x66a9893cc07d91d95644aedd05d03f95e1dba8af), value: 0, access_list: AccessList([]), input: 0x3593564c000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000000a0000>
Dec 03 17:22:25 arch reth[2585726]: ned { tx: TxLegacy { chain_id: Some(1), nonce: 12, gas_price: 22883114, gas_limit: 91008, to: Call(0xdac17f958d2ee523a2206206994597c13d831ec7), value: 0, input: 0xa9059cbb000000000000000000000000708c218d548c524d801f0137d16a2121048ddb180000000000000000000000000000000000000000000000000000000000001388 }, signature: Signature { y_parity: false, r: 73088093242269909114809257083045734>
Dec 03 17:22:25 arch reth[2585726]: 595495200387046983517265 }, hash: OnceLock(<uninit>) }), Legacy(Signed { tx: TxLegacy { chain_id: Some(1), nonce: 1, gas_price: 22011376, gas_limit: 21000, to: Call(0xd59df0bd79d39b45324c033eab6a323f44d32b5e), value: 169515569000, input: 0x }, signature: Signature { y_parity: true, r: 100391635375663370633849946315604822416263709603721751145865659702717283438541, s: 203565024247>
Dec 03 17:22:25 arch reth[2585726]: 0000000000000000000000000000000000297fbe1f4da7613c79982e0decc48fbb56101fc919bc341d84e7afdecf2d4f007e017c00000000000000000000000000000000000000000000000000000000000002f5000000000000000000000000000000000000000000000000000000000000002a4b0f546f19b57b53c362a3957c398a6cb0dd4e208ab83caff0a99fe53b0a42000000000000000000000000000000000000000000000000000000000000000120d0c561e22208c1d2a2cc5>
Dec 03 17:22:25 arch reth[2585726]: 74c33c6bf6fbcd38baab813cbb6, amount: 17775650 }, Withdrawal { index: 110414044, validator_index: 1581395, address: 0xd007058e9b58e74c33c6bf6fbcd38baab813cbb6, amount: 17745062 }, Withdrawal { index: 110414045, validator_index: 1581396, address: 0xd007058e9b58e74c33c6bf6fbcd38baab813cbb6, amount: 63784913 }, Withdrawal { index: 110414046, validator_index: 1581397, address: 0xd007>
Dec 03 17:22:25 arch reth[2585726]: 2025-12-03T22:22:25.537997Z  WARN Invalid block error on new payload invalid_hash=0x3b28163fd808f8febb46b220b7bf588e0ac1039f22ac81a7c3c017f622bd15b8 invalid_number=23935839 validation_err=invalid excess blob gas: got 32942754, expected 33117516; parent excess blob gas: 32811682, parent blob gas used: 917504
Dec 03 17:22:25 arch reth[2585726]: 2025-12-03T22:22:25.538034Z  WARN Bad block with hash invalid_ancestor=BlockWithParent { parent: 0x6b5bfe7e65085295b7a1b74ae61cda2fa2d27646ea7519df81bf038106e3bdbb, block: NumHash { number: 23935839, hash: 0x3b28163fd808f8febb46b220b7bf588e0ac1039f22ac81a7c3c017f622bd15b8 } }
Dec 03 17:22:25 arch reth[2585726]: 2025-12-03T22:22:25.538234Z  WARN Encountered invalid block number=23935839 hash=0x3b28163fd808f8febb46b220b7bf588e0ac1039f22ac81a7c3c017f622bd15b8
Dec 03 17:22:38 arch reth[2585726]: 2025-12-03T22:22:38.981962Z  INFO Received block from consensus engine number=23935839 hash=0xcc821eba3d41c44d8bdfacfd1740563042e33b9c5393779d532678c6f7c2580a
Dec 03 17:22:39 arch reth[2585726]: 2025-12-03T22:22:39.655880Z  INFO Regular root task finished regular_state_root=0x0bd2a7f14f73d8f3222b8062d30bddc18c11bbbf80fcb470a95f1b205ef80248 elapsed=563.396989ms
.....
Dec 03 17:25:24 arch reth[2585726]: 2025-12-03T22:25:24.972921Z  INFO Regular root task finished regular_state_root=0xa80b8189f78de3eb38f0451922f11f10ff5eb63646ae929a6bceeb4e76e78e07 elapsed=254.829966ms
Dec 03 17:25:24 arch reth[2585726]: 2025-12-03T22:25:24.973771Z  INFO Block added to canonical chain number=23935853 hash=0x184f36277a5f07d7628e92bed2eb801127371aefb1a625f0ff4ad8d7f0ed4a1e peers=131 txs=217 gas_used=30.90Mgas gas_throughput=103.03Mgas/second gas_limit=60.00Mgas full=51.5% base_fee=0.02Gwei blobs=0 excess_blobs=271 elapsed=299.942668ms
Dec 03 17:25:25 arch reth[2585726]: 2025-12-03T22:25:25.158979Z  INFO Canonical chain committed number=23935853 hash=0x184f36277a5f07d7628e92bed2eb801127371aefb1a625f0ff4ad8d7f0ed4a1e elapsed=400.299µs
Dec 03 17:25:40 arch reth[2585726]: 2025-12-03T22:25:40.029307Z  INFO Received block from consensus engine number=23935809 hash=0xd6cbb630c84c44396a2360d271a9f9f0c8b4c3bc13a442b6c418ad3bea8b8cc4
Dec 03 17:26:07 arch systemd[1]: reth.service: A process of this unit has been killed by the OOM killer.
Dec 03 17:26:10 arch systemd[1]: reth.service: Main process exited, code=killed, status=9/KILL
Dec 03 17:26:10 arch systemd[1]: reth.service: Failed with result 'oom-kill'.
Dec 03 17:26:10 arch systemd[1]: reth.service: Consumed 14h 29min 15.151s CPU time, 20.2G memory peak.
Dec 03 17:26:15 arch systemd[1]: reth.service: Scheduled restart job, restart counter is at 4.
Dec 03 17:26:15 arch systemd[1]: Started Reth Execution Client.
Dec 03 17:26:15 arch reth[2673735]: 2025-12-03T22:26:15.571588Z  INFO Initialized tracing, debug log directory: xxxxx/mainnet/reth/mainnet
Dec 03 17:26:15 arch reth[2673735]: 2025-12-03T22:26:15.576096Z  INFO Starting reth version="1.9.3 (27a8c0f)"
Dec 03 17:26:15 arch reth[2673735]: 2025-12-03T22:26:15.576773Z  INFO Opening database path="xxxxx/mainnet/reth/db"
Dec 03 17:26:15 arch reth[2673735]: 2025-12-03T22:26:15.594376Z  INFO Launching node
Dec 03 17:26:15 arch reth[2673735]: 2025-12-03T22:26:15.604891Z  INFO Configuration loaded path="xxxxx/mainnet/reth/reth.toml"
Dec 03 17:26:15 arch reth[2673735]: 2025-12-03T22:26:15.630353Z  INFO Verifying storage consistency.
Dec 03 17:26:15 arch reth[2673735]: 2025-12-03T22:26:15.650703Z  INFO Database opened
Dec 03 17:26:15 arch reth[2673735]: 2025-12-03T22:26:15.651413Z  INFO Starting metrics endpoint at 127.0.0.1:9001
Dec 03 17:26:15 arch reth[2673735]: 2025-12-03T22:26:15.654441Z  INFO
Dec 03 17:26:15 arch reth[2673735]: Pre-merge hard forks (block based):
Dec 03 17:26:15 arch reth[2673735]: - Frontier                         @0
Dec 03 17:26:15 arch reth[2673735]: - Homestead                        @1150000
Dec 03 17:26:15 arch reth[2673735]: - Dao                              @1920000
Dec 03 17:26:15 arch reth[2673735]: - Tangerine                        @2463000
Dec 03 17:26:15 arch reth[2673735]: - SpuriousDragon                   @2675000
Dec 03 17:26:15 arch reth[2673735]: - Byzantium                        @4370000
Dec 03 17:26:15 arch reth[2673735]: - Constantinople                   @7280000
Dec 03 17:26:15 arch reth[2673735]: - Petersburg                       @7280000
Dec 03 17:26:15 arch reth[2673735]: - Istanbul                         @9069000
Dec 03 17:26:15 arch reth[2673735]: - MuirGlacier                      @9200000
Dec 03 17:26:15 arch reth[2673735]: - Berlin                           @12244000
Dec 03 17:26:15 arch reth[2673735]: - London                           @12965000
Dec 03 17:26:15 arch reth[2673735]: - ArrowGlacier                     @13773000
Dec 03 17:26:15 arch reth[2673735]: - GrayGlacier                      @15050000
Dec 03 17:26:15 arch reth[2673735]: Merge hard forks:
Dec 03 17:26:15 arch reth[2673735]: - Paris                            @58750000000000000000000 (network is known to be merged)
Dec 03 17:26:15 arch reth[2673735]: Post-merge hard forks (timestamp based):
Dec 03 17:26:15 arch reth[2673735]: - Shanghai                         @1681338455
Dec 03 17:26:15 arch reth[2673735]: - Cancun                           @1710338135          blob: (target: 3, max: 6, fraction: 3338477)
Dec 03 17:26:15 arch reth[2673735]: - Prague                           @1746612311          blob: (target: 6, max: 9, fraction: 5007716)
Dec 03 17:26:15 arch reth[2673735]: - Osaka                            @1764798551          blob: (target: 6, max: 9, fraction: 5007716)
Dec 03 17:26:15 arch reth[2673735]: - Bpo1                             @1765290071          blob: (target: 10, max: 15, fraction: 8346193)
Dec 03 17:26:15 arch reth[2673735]: - Bpo2                             @1767747671          blob: (target: 14, max: 21, fraction: 11684671)
Dec 03 17:26:15 arch reth[2673735]: 2025-12-03T22:26:15.664527Z  INFO Transaction pool initialized
```

### Platform(s)

Linux (x86)

### Container Type

Not running in a container

### What version/commit are you on?

```
Reth Version: 1.9.3
Commit SHA: 27a8c0f5a6dfb27dea84c5751776ecabdd069646
Build Timestamp: 2025-11-20T15:10:06.385593747Z
Build Features: asm_keccak,jemalloc,otlp
Build Profile: maxperf
```

### What database version are you on?

Available on request.

### Which chain / network are you on?

Mainnet

### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

None

### If you've built Reth from source, provide the full command you used

RUSTFLAGS="-C target-cpu=native" cargo build --profile maxperf

### Code of Conduct

- [x] I agree to follow the Code of Conduct
