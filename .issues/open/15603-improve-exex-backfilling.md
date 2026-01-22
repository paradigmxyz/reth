---
title: Improve exex backfilling
labels:
    - A-exex
    - C-enhancement
    - M-prevent-stale
    - S-needs-design
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:15.985511Z
info:
    author: chrisdotn
    created_at: 2025-04-08T10:47:04Z
    updated_at: 2025-06-16T07:53:11Z
---

### Describe the feature

I'm using exex to create an indexer for the blockchains for specific data. I want it to first backfill from genesis so that I have the full data in my data sink and then continue onwards and keep syncing. To first backfill, I start the exex and set its `start_block` to 0. Then it starts backfilling. 

During backfill, new blocks arrive clogging up the WAL list, which can get quite large. 

The current workaround is to start the exex with `--trusted-only` to prevent it from finding peers. Once the exex has finished the backfill, restart it w/o `--trusted-only`, let it catch up to the current head and continue onwards. 

It would be nice if there is a better way of handling the particular time delay when backfilling an entire chain. The main idea would be to have the exex identify if it is backfilling and just remember the current head as `exex_start_block` when it was started. Once the backfill is through, sync up from the `exex_start_block` to head and then continue on-wards. 

A (simplified) example for the actual indexing: 

```rust
#[derive(serde::Serialize)]
pub struct EmitterBlock {
    pub hash: FixedBytes<32>,
    pub header: Header,
    pub transactions: Vec<EmitterTransaction>,
}

#[derive(serde::Serialize)]
pub struct EmitterTransaction {
    pub hash: FixedBytes<32>,
    pub from: Address,
    pub to: Address,
    pub value: U256,
    pub input: Bytes,
    pub receipt: EmitterReceipt,
}

#[derive(serde::Serialize)]
pub struct EmitterReceipt {
    pub success: bool,
    pub logs: Vec<Log<LogData>>,
}
fn process_committed_chain(new: &Chain) {
    let mut receipts_iter = new.receipts_with_attachment().into_iter();

    // serialize each block
    for block in new.blocks_iter() {
        let receipts = receipts_iter
            .find(|block_receipt| { block_receipt.block.number == block.number })
            .unwrap();

        let emitter_block = process_committed_block(block, receipts);

        info!(
            block_number = block.number,
            block = serde_json::to_string(&emitter_block).unwrap(),
            "New block"
        );
    }
}

fn process_committed_block(
    block: &reth::primitives::RecoveredBlock<alloy::consensus::Block<TransactionSigned>>,
    receipts: BlockReceipts
) -> EmitterBlock {
    let transactions = block.transactions_recovered().map(|tx| {
        let tx_hash = tx.hash();
        // get receipt for tx
        let (_, tx_receipt) = receipts.tx_receipts
            .iter()
            .find(|(tx_id, _)| { tx_id == tx_hash })
            .unwrap();

        EmitterTransaction {
            hash: tx_hash.clone(),
            from: tx.signer(),
            to: tx.to().unwrap_or_default(),
            value: tx.value(),
            input: tx.input().clone(),
            receipt: EmitterReceipt {
                success: tx_receipt.success,
                logs: tx_receipt.logs.clone(),
            },
        }
    });

    EmitterBlock {
        hash: block.hash(),
        header: block.header().clone(),
        transactions: transactions.collect(),
    }
}
```

### Additional context

_No response_
