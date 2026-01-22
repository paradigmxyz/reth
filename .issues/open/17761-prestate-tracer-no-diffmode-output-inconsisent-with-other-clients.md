---
title: Prestate tracer (no diffmode) output inconsisent with other clients
labels:
    - A-rpc
    - C-bug
assignees:
    - rakita
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:15.994385Z
info:
    author: mirgee
    created_at: 2025-08-08T07:33:28Z
    updated_at: 2025-10-20T12:09:58Z
---

### Describe the bug

The response to
```
curl -X POST --data '{"jsonrpc":"2.0","method":"debug_traceTransaction","params":["0x6faccb4e28da1bc309421edb3aed19ce6b7409b83acf20cf4a0fc7748d7a4fb0",{"tracer": "prestateTracer", "tracerConfig": {"diffMode": false}}], "id":1}' $NODE_URL -H "Content-Type: application/json"
```
differs between Erigon, Geth and Reth.

More specifically, the Reth response includes accesses from the EIP-2930 `accessList` field of the transaction, whereas Erigon and Geth doesn't.

Cross-checking against the output of the default tracer on Reth and Besu
```
curl -X POST --data '{"jsonrpc":"2.0","method":"debug_traceTransaction","params":["0x6faccb4e28da1bc309421edb3aed19ce6b7409b83acf20cf4a0fc7748d7a4fb0",{"disableStorage":false,"disableMemory":false,"disableStack":false}], "id":1}' $NODE_URL -H "Content-Type: application/json"
```
reveals that indeed the slots were not touched during transaction execution.

**Reth version**: `reth/v1.6.0-d8451e5/x86_64-unknown-linux-gnu`

**Note**: The queried Reth node is run by Alchemy.

[transaction-0x6faccb.json](https://github.com/user-attachments/files/21679111/transaction-0x6faccb.json)
[transaction-trace-erigon-0x6faccb.json](https://github.com/user-attachments/files/21679114/transaction-trace-erigon-0x6faccb.json)
[transaction-trace-geth-0x6faccb.json](https://github.com/user-attachments/files/21679110/transaction-trace-geth-0x6faccb.json)
[transaction-trace-reth-0x6faccb.json](https://github.com/user-attachments/files/21679112/transaction-trace-reth-0x6faccb.json)
[transaction-trace-reth-default-0x6faccb.json](https://github.com/user-attachments/files/21679113/transaction-trace-reth-default-0x6faccb.json)

### Steps to reproduce

Run
```
curl -X POST --data '{"jsonrpc":"2.0","method":"debug_traceTransaction","params":["0x6faccb4e28da1bc309421edb3aed19ce6b7409b83acf20cf4a0fc7748d7a4fb0",{"tracer": "prestateTracer", "tracerConfig": {"diffMode": false}}], "id":1}' $NODE_URL -H "Content-Type: application/json"
```
against different clients and compare results.

### Node logs

```text

```

### Platform(s)

_No response_

### Container Type

Other

### What version/commit are you on?

The output of
```
curl -X POST --data '{"jsonrpc":"2.0","id":1,"method":"web3_clientVersion","params":[]}' $NODE_URL -H "Content-Type: application/json"
```
against the Alchemy node is
 ```
reth/v1.6.0-d8451e5/x86_64-unknown-linux-gnu
```

### What database version are you on?

Don't know.

### Which chain / network are you on?

Mainnet.

### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

_No response_

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [x] I agree to follow the Code of Conduct
