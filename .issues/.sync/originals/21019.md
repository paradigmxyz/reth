---
title: '`poll_imported_transactions` is not executing so RPC transactions don''t propagate to peers'
labels:
    - C-bug
    - S-needs-triage
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.019289Z
info:
    author: galimba
    created_at: 2026-01-14T07:15:01Z
    updated_at: 2026-01-14T13:01:32Z
---

### Describe the bug

Under certain conditions `poll_imported_transactions` never executes, so RPC transactions don't propagate to peers. 

Transactions sent via `eth_sendRawTransaction` get accepted into the local txpool but never gossip out to the network. We've been debugging this on our private testnet for a few days, and the key finding is this metric stays at zero permanently:
```
reth_network_acc_duration_poll_imported_transactions 0
```

Meanwhile `reth_network_acc_duration_poll_transaction_events` shows non-zero values, so the TransactionsManager is running - it's just not receiving notifications for locally submitted transactions.

Also noticed that `messages_sent` always equals `messages_received`, meaning the node only responds to peer requests and never initiates tx broadcasts.

I used Claude to check for similar issues and found PR #11675 addressed something similar in v1.1.1, so either there's a regression or this is triggered by a specific config.

**Workaround**: Submitting raw txs directly to a validator works fine - gets mined and syncs back, confirming P2P is functional. We hit this because we're running validators privately with a dedicated RPC node as the public entry point using `--disable-discovery` + `--trusted-peers`.  I suspect this setup isn't common and triggers the bug.

### Steps to reproduce

1. Set up a private PoS network with multiple Reth nodes (validators + RPC node)
2. Start the RPC node **with discovery disabled**:
```bash
reth node \
  --chain /path/to/genesis.json \
  --datadir /reth \
  --nat none \
  --disable-discovery \
  --tx-propagation-mode all \
  --no-persist-peers \
  --trusted-peers "enode://PUBKEY1@VALIDATOR1_IP:30303,enode://PUBKEY2@VALIDATOR2_IP:30303" \
  --http --http.api "eth,web3,debug,net,txpool,trace,rpc"
```
3. Wait for peers to connect
4. Submit a transaction via `eth_sendRawTransaction`
5. Check `txpool_status` on the RPC node, **transaction is there**
6. Check `txpool_status` on validators, **pools are empty**
7. Blocks get mined empty while the RPC node has pending txs

I tried `--tx-propagation-mode sqrt` as well, same result. Issue persists across restarts and is present immediately after startup.



### Node logs

```text
No errors related to tx propagation. Relevant metrics:

$ curl -s localhost:9080/metrics | grep acc_duration_poll_imported
reth_network_acc_duration_poll_imported_transactions 0

$ curl -s localhost:9080/metrics | grep acc_duration_poll_transaction_events  
reth_network_acc_duration_poll_transaction_events 0.000000238

$ curl -s localhost:9080/metrics | grep pool_transactions_messages
reth_network_pool_transactions_messages_sent_total 983
reth_network_pool_transactions_messages_received_total 983


Txpool comparison between RPC node and validator:

# RPC node
{"result":{"pending":"0x9","queued":"0x0"}}

# Validator
{"result":{"pending":"0x0","queued":"0x0"}}
```

### Platform(s)

Linux (x86)

### Container Type

Docker

### What version/commit are you on?

v1.9.3 (`ghcr.io/paradigmxyz/reth:v1.9.3`)

### What database version are you on?

2

### Which chain / network are you on?

Private network (custom genesis.json)

### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

_No response_

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [x] I agree to follow the Code of Conduct
