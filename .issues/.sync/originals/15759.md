---
title: Confusing OP reth eth_sendRawTransaction forwarding
labels:
    - A-op-reth
    - A-rpc
    - C-bug
    - D-good-first-issue
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.986297Z
info:
    author: rrrengineer
    created_at: 2025-04-15T21:28:54Z
    updated_at: 2025-09-10T16:46:18Z
---

### Describe the bug

Current flow:
- user sends eth_sendRawTransaction to op reth node
- op reth node forwards tx to sequencer no matter what happens.
- Then op reth does tx verification, calculates hash and generates response.

It might bring 2 possible confusing situations.

Situation 1: 
- sequencer declines transaction
- op-reth returns success json response with generated tx hash
- user got confused, not seeing transaction in etherscan.io

Situation 2: 
- sequencer accepts transaction
- op-reth verification failed (due to node different configuration or other reasons) and returns error json response
- user got confused, software thinks tx failed but it went through.


Current implementation:
```
        // On optimism, transactions are forwarded directly to the sequencer to be included in
        // blocks that it builds.
        if let Some(client) = self.raw_tx_forwarder().as_ref() {
            tracing::debug!(target: "rpc::eth", hash = %pool_transaction.hash(), "forwarding raw transaction to sequencer");
            let _ = client.forward_raw_transaction(&tx).await.inspect_err(|err| {
                    tracing::debug!(target: "rpc::eth", %err, hash=% *pool_transaction.hash(), "failed to forward raw transaction");
                });
        }

        // submit the transaction to the pool with a `Local` origin
        let hash = self
            .pool()
            .add_transaction(TransactionOrigin::Local, pool_transaction)
            .await
            .map_err(Self::Error::from_eth_err)?;

        Ok(hash)
```

Proposed solutions:
forward sequencer response back to user as well.	

### Steps to reproduce

- Start op-reth full node with broken sequencer link or any rest api that emulates RPC error response.
- Run any transaction eth_sendRawTransaction to reth full node.

As a result "sequencer" declines transaction `target=rpc::sequencer message="HTTP request to sequencer failed"` in a background.
op-reth goes with its own verification and success response.

### Node logs

```text

```

### Platform(s)

_No response_

### Container Type

Not running in a container

### What version/commit are you on?

latest

### What database version are you on?

latest

### Which chain / network are you on?

optimism, base

### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

_No response_

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [x] I agree to follow the Code of Conduct
