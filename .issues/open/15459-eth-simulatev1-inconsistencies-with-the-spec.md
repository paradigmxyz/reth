---
title: eth_simulateV1 inconsistencies with the spec
labels:
    - A-rpc
    - C-bug
    - C-tracking-issue
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.984318Z
info:
    author: mralj
    created_at: 2025-04-02T13:47:42Z
    updated_at: 2025-04-25T09:59:31Z
---

### Describe the bug

I have noticed following inconsistencies with the official spec when using `eth_simulateV1`:
1. Unless block gas limit is set manually the reth node erros with: `{"jsonrpc":"2.0","id":1,"error":{"code":-38026,"message":"Client adjustable limit reached"}}`
2. If `validation=true`, the following fields are required to be explicitly provided in `call` (transaction): `to`, `nonce`, `gasLimit`,  and depending on TX type `gasPrice`/ EIP 1559 gas price fields. There is no such requirement in spec, and both geth and NMC work w/o these fields
3. `call` expect field controlling gas limit to be called `gas` and spec has this field named `gasLimit`. Not sure if this is an actuall issue, since both NMC and geth also call this field `gas` (I've left comment about this on [eth_simulate PR](https://github.com/ethereum/execution-apis/pull/484))

And regarding point 2, I think your approach makes sense (this is why I written in the `eth_simulate` TG group about this), but it is inconsistent with other implementations and spec. 

To sum up point 1 is I think bug for sure, and point 2 and 3 are debatable :)

### Steps to reproduce

Have `reth` node with RPC enabled:

```Bash
curl http://127.0.0.1:8545/ \
  -X POST \
  -H "Content-Type: application/json" \
  --data '{
    "jsonrpc": "2.0",
    "method": "eth_simulateV1",
    "params": [
      {
        "blockStateCalls": [
          {
            "stateOverrides": {
              "0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045": {
                "balance": "0x56BC75E2D63100000"
              }
            },
            "calls": [
              {
                "from": "0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045",
                "to": "0x014d023e954bAae7F21E56ed8a5d81b12902684D",
                "maxFeePerGas": "0x258",
                "value": "0x1"
              },
              {
                "from": "0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045",
                "to": "0x014d023e954bAae7F21E56ed8a5d81b12902684D",
                "maxFeePerGas": "0x258",
                "value": "0x1"
              }
            ]
          }
        ],
        "validation": true,
        "traceTransfers": true
      },
      "latest"
    ],
    "id": 1
  }'
```

Fails with:

```
{"jsonrpc":"2.0","id":1,"error":{"code":-38026,"message":"Client adjustable limit reached"}}
```

If I provide `gasLimit` then it will fail for another reason:

```BASH
curl http://127.0.0.1:8545/ \
  -X POST \
  -H "Content-Type: application/json" \
  --data '{
    "jsonrpc": "2.0",
    "method": "eth_simulateV1",
    "params": [
      {
        "blockStateCalls": [
          {
            "blockOverrides": {
              "baseFeePerGas": "0x9",
              "gasLimit": "0x2e631"
            },
            "stateOverrides": {
              "0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045": {
                "balance": "0x56BC75E2D63100000"
              }
            },
            "calls": [
              {
                "from": "0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045",
                "to": "0x014d023e954bAae7F21E56ed8a5d81b12902684D",
                "maxFeePerGas": "0x258",
                "value": "0x1"
              },
              {
                "from": "0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045",
                "to": "0x014d023e954bAae7F21E56ed8a5d81b12902684D",
                "maxFeePerGas": "0x258",
                "value": "0x1"
              }
            ]
          }
        ],
        "validation": true,
        "traceTransfers": true
      },
      "latest"
    ],
    "id": 1
  }'
```

ERROR:
```
{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"Transaction conversion error"}}
```


Expected output:

```
{"jsonrpc":"2.0","id":1,"result":[{"baseFeePerGas":"0x9","blobGasUsed":"0x0","calls":[{"returnData":"0x","logs":[{"address":"0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","topics":["0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef","0x000000000000000000000000d8da6bf26964af9d7eed9e03e53415d37aa96045","0x000000000000000000000000014d023e954baae7f21e56ed8a5d81b12902684d"],"data":"0x0000000000000000000000000000000000000000000000000000000000000001","blockNumber":"0x16cf07c","transactionHash":"0xc8668d7432dcd314a28f49c09a9cc9928e8a9b607f9848ee019986e4356ce32c","transactionIndex":"0x0","blockHash":"0x06020b5490170ea024bb8ca8b965074398a7d28a1416bde587d77d1ecf68e504","logIndex":"0x0","removed":false}],"gasUsed":"0x5208","status":"0x1"},{"returnData":"0x","logs":[{"address":"0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","topics":["0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef","0x000000000000000000000000d8da6bf26964af9d7eed9e03e53415d37aa96045","0x000000000000000000000000014d023e954baae7f21e56ed8a5d81b12902684d"],"data":"0x0000000000000000000000000000000000000000000000000000000000000001","blockNumber":"0x16cf07c","transactionHash":"0xe4d567a06c3892a33043db4c46fa4af266cc3823ca28166e3bac2b152eae1330","transactionIndex":"0x1","blockHash":"0x06020b5490170ea024bb8ca8b965074398a7d28a1416bde587d77d1ecf68e504","logIndex":"0x1","removed":false}],"gasUsed":"0x5208","status":"0x1"}],"difficulty":"0x0","excessBlobGas":"0x0","extraData":"0x","gasLimit":"0x2e631","gasUsed":"0xa410","hash":"0x06020b5490170ea024bb8ca8b965074398a7d28a1416bde587d77d1ecf68e504","logsBloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","miner":"0x4200000000000000000000000000000000000011","mixHash":"0x0000000000000000000000000000000000000000000000000000000000000000","nonce":"0x0000000000000000","number":"0x16cf07c","parentBeaconBlockRoot":"0x0000000000000000000000000000000000000000000000000000000000000000","parentHash":"0xe38f6ccdd54be08167884b0ff937c4e6de0c832f50450e1c478b0d2c1068f556","receiptsRoot":"0x75308898d571eafb5cd8cde8278bf5b3d13c5f6ec074926de3bb895b519264e1","sha3Uncles":"0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347","size":"0x29f","stateRoot":"0x1f35f1a0b2f5e829f701b186cf15153bc0258f619ce336d4850cb4abc75f47f7","timestamp":"0x67ed3fe2","transactions":["0xc8668d7432dcd314a28f49c09a9cc9928e8a9b607f9848ee019986e4356ce32c","0xe4d567a06c3892a33043db4c46fa4af266cc3823ca28166e3bac2b152eae1330"],"transactionsRoot":"0x84a852ee7845263f641d071b02f898f05a594a40e188817b9673adf200761fb0","uncles":[],"withdrawals":[],"withdrawalsRoot":"0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421"}]}
```

### Platform(s)

Linux (x86)

### Container Type

Kubernetes

### What version/commit are you on?

```
op-reth --version
reth-optimism-cli Version: 1.3.4
Commit SHA: 90c514ca818a36eb8cd36866156c26a4221e9c4a
Build Timestamp: 2025-03-21T19:27:52.553479860Z
Build Features: asm_keccak,jemalloc
Build Profile: maxperf
```

### What database version are you on?

```
op-reth db version

thread 'main' panicked at crates/tracing/src/layers.rs:136:46:
Could not create log directory: Os { code: 30, kind: ReadOnlyFilesystem, message: "Read-only file system" }
stack backtrace:
note: Some details are omitted, run with `RUST_BACKTRACE=full` for a verbose backtrace.
```

### Which chain / network are you on?

```
op-reth --chain
error: a value is required for '--chain <CHAIN_OR_PATH>' but none was supplied

For more information, try '--help'.
```

### What type of node are you running?

Full via --full flag

### Code of Conduct

- [x] I agree to follow the Code of Conduct
