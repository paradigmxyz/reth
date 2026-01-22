---
title: No 'root' field for eth_getTransactionReceipt and eth_getBlockReceipts in blocks before Byzantium fork in Ethereum Mainnet
labels:
    - A-rpc
    - C-bug
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.018944Z
info:
    author: AlexVarin2001
    created_at: 2026-01-13T15:55:48Z
    updated_at: 2026-01-16T12:13:09Z
---

### Describe the bug

We face with incorrect RPC responses associated with Ethereum mainnet at blocks 4369992-4369999 (just before the Byzantium fork). The eth_getTransactionReceipt and eth_getBlockReceipts calls both show receipts containing status fields, but no root fields. All blocks before the fork at 4370000 should contain the root. Other clients (Erigon, Geth, Nethermind) contains this field.

### Steps to reproduce

1) Send request to node:
```
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "eth_getBlockReceipts",
  "params": ["0x42ae49"]
}
```

2) Get answer with no 'root' field
```
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": [
    {
      "type": "0x0",
      "status": "0x1",
      "cumulativeGasUsed": "0x9e39",
      "logs": [
        {
          "address": "0x27695e09149adc738a978e9a678f99e4c39e9eb9",
          "topics": [
            "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef",
            "0x000000000000000000000000bf0976655b00971e14c918756aa69914f2595e05",
            "0x0000000000000000000000006d42c83646a04d060f6dc203087b069696ce976d"
          ],
          "data": "0x0000000000000000000000000000000000000000000000000000078229b6f8c0",
          "blockHash": "0x227058163aa138554d10505effef404618a4915b1a138d6f8c5df32beda0c87b",
          "blockNumber": "0x42ae49",
          "blockTimestamp": "0x59e44155",
          "transactionHash": "0x97e755279bfdeaddb8b9591cf317531eeac79d3b15d4d1ae86f250a792e8919f",
          "transactionIndex": "0x0",
          "logIndex": "0x0",
          "removed": false
        }
      ],
      "logsBloom": "0x00000000000000002000001000000000000000000000000000000000000000000000000000000000000000000400000000000000000000000000000000000100000000000000000000000008000000000000000000000000000000080000000000000000000000000000000000000000000000000000000000000010000000000000000000080000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000004000020000000000000000000000000000000000000000000080000000000000",
      "transactionHash": "0x97e755279bfdeaddb8b9591cf317531eeac79d3b15d4d1ae86f250a792e8919f",
      "transactionIndex": "0x0",
      "blockHash": "0x227058163aa138554d10505effef404618a4915b1a138d6f8c5df32beda0c87b",
      "blockNumber": "0x42ae49",
      "gasUsed": "0x9e39",
      "effectiveGasPrice": "0xba43b7400",
      "from": "0xbf0976655b00971e14c918756aa69914f2595e05",
      "to": "0x27695e09149adc738a978e9a678f99e4c39e9eb9",
      "contractAddress": null
    },
```
3) Expected 'root' field contains (for example same request on Geth):

```
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": [
    {
      "blockHash": "0x227058163aa138554d10505effef404618a4915b1a138d6f8c5df32beda0c87b",
      "blockNumber": "0x42ae49",
      "contractAddress": null,
      "cumulativeGasUsed": "0x9e39",
      "effectiveGasPrice": "0xba43b7400",
      "from": "0xbf0976655b00971e14c918756aa69914f2595e05",
      "gasUsed": "0x9e39",
      "logs": [
        {
          "address": "0x27695e09149adc738a978e9a678f99e4c39e9eb9",
          "topics": [
            "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef",
            "0x000000000000000000000000bf0976655b00971e14c918756aa69914f2595e05",
            "0x0000000000000000000000006d42c83646a04d060f6dc203087b069696ce976d"
          ],
          "data": "0x0000000000000000000000000000000000000000000000000000078229b6f8c0",
          "blockNumber": "0x42ae49",
          "transactionHash": "0x97e755279bfdeaddb8b9591cf317531eeac79d3b15d4d1ae86f250a792e8919f",
          "transactionIndex": "0x0",
          "blockHash": "0x227058163aa138554d10505effef404618a4915b1a138d6f8c5df32beda0c87b",
          "blockTimestamp": "0x59e44155",
          "logIndex": "0x0",
          "removed": false
        }
      ],
      "logsBloom": "0x00000000000000002000001000000000000000000000000000000000000000000000000000000000000000000400000000000000000000000000000000000100000000000000000000000008000000000000000000000000000000080000000000000000000000000000000000000000000000000000000000000010000000000000000000080000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000004000020000000000000000000000000000000000000000000080000000000000",
      "root": "0xb9709115a77033b36df55b80d72b84425034d4549d089accd27a3dd9ba2f993b",
      "to": "0x27695e09149adc738a978e9a678f99e4c39e9eb9",
      "transactionHash": "0x97e755279bfdeaddb8b9591cf317531eeac79d3b15d4d1ae86f250a792e8919f",
      "transactionIndex": "0x0",
      "type": "0x0"
    },
```

### Node logs

```text

```

### Platform(s)

Linux (x86)

### Container Type

Docker

### What version/commit are you on?

ghcr.io/paradigmxyz/reth:v1.9.3

### What database version are you on?

-

### Which chain / network are you on?

Ethereum Mainnet

### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

_No response_

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [x] I agree to follow the Code of Conduct
