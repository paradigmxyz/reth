//! Bounded in-process checkpoint reads feeding native execution workers.

use super::store::{self, Catalog, Record, Store, BYTE_BUDGET};
use bytes::Bytes;
use crossbeam_channel::{RecvTimeoutError, Sender, TrySendError};
use parking_lot::Mutex;
use reth_cli_util::cancellation::CancellationToken;
use reth_consensus::FullConsensus;
use reth_evm::ConfigureEvm;
use slatedb::{config::ScanOptions, DbReader};
use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
};
use tracing::info;

struct Ready {
    record: Record,
    bytes: Bytes,
    // Acquired before fetching, held through decoding and execution. The Bytes
    // value moves between tasks without a framing copy or an input mutex.
    _reservation: OwnedSemaphorePermit,
}

pub(super) fn run<E, C>(
    store: Store,
    catalog: Catalog,
    records: Vec<Record>,
    config: E,
    consensus: C,
    workers: usize,
    cancellation: CancellationToken,
) -> eyre::Result<()>
where
    E: ConfigureEvm,
    C: FullConsensus<E::Primitives> + Send + Sync,
{
    eyre::ensure!(workers > 0, "num-tasks must be positive");
    let runtime =
        tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build()?;
    let reader = Arc::new(runtime.block_on(store.reader(&catalog))?);
    let budget = Arc::new(Semaphore::new(BYTE_BUDGET));
    let (sender, receiver) = crossbeam_channel::bounded::<Ready>(4);
    let started = Instant::now();
    // Save the triggering error before cancelling peers. Join order must not
    // replace missing/corrupt state with a secondary cancellation error.
    let failure = Mutex::new(None);
    let result = std::thread::scope(|scope| -> eyre::Result<()> {
        let producer = scope.spawn(|| {
            if let Err(error) =
                runtime.block_on(feed(&reader, &records, sender, budget.clone(), &cancellation))
            {
                failure.lock().get_or_insert(error);
                cancellation.cancel();
                budget.close();
            }
        });
        let mut threads = Vec::new();
        for _ in 0..workers {
            let receiver = receiver.clone();
            let config = &config;
            let consensus = &consensus;
            let cancellation = &cancellation;
            let budget = &budget;
            let failure = &failure;
            threads.push(scope.spawn(move || {
                let result = (|| -> eyre::Result<Vec<(u64, u64, u64)>> {
                    let mut completed = Vec::new();
                    loop {
                        eyre::ensure!(!cancellation.is_cancelled(), "corpus worker cancelled");
                        let ready = match receiver.recv_timeout(Duration::from_millis(100)) {
                            Ok(ready) => ready,
                            Err(RecvTimeoutError::Timeout) => continue,
                            Err(RecvTimeoutError::Disconnected) => break,
                        };
                        let chunk = store::decode(&ready.bytes, &ready.record)?;
                        let (transactions, gas) = chunk.replay(config, consensus, cancellation)?;
                        eyre::ensure!(
                            (transactions, gas) == (ready.record.transactions, ready.record.gas),
                            "corpus transaction/gas totals mismatch"
                        );
                        completed.push((ready.record.start, transactions, gas));
                        // Drop decoded state, serialized Bytes, and its permit
                        // together, before receiving more work.
                    }
                    Ok(completed)
                })();
                match result {
                    Ok(items) => items,
                    Err(error) => {
                        failure.lock().get_or_insert(error);
                        cancellation.cancel();
                        budget.close();
                        Vec::new()
                    }
                }
            }));
        }
        drop(receiver);
        let mut completed = Vec::new();
        for worker in threads {
            match worker.join() {
                Ok(items) => completed.extend(items),
                Err(_) => {
                    failure.lock().get_or_insert_with(|| eyre::eyre!("corpus worker panicked"));
                    cancellation.cancel();
                    budget.close();
                }
            }
        }
        producer.join().map_err(|_| eyre::eyre!("corpus reader panicked"))?;
        if let Some(error) = failure.lock().take() {
            return Err(error);
        }
        let wanted: BTreeSet<_> = records.iter().map(|r| r.start).collect();
        let got: BTreeSet<_> = completed.iter().map(|r| r.0).collect();
        eyre::ensure!(
            completed.len() == wanted.len() && got == wanted,
            "missing/duplicate completed corpus chunks"
        );
        let transactions: u64 = completed.iter().map(|r| r.1).sum();
        let gas: u64 = completed.iter().map(|r| r.2).sum();
        let blocks: u64 = records.iter().map(|r| r.end - r.start).sum();
        info!(
            blocks,
            transactions,
            gas,
            chunks = completed.len(),
            workers,
            seconds = started.elapsed().as_secs_f64(),
            serialized_budget = BYTE_BUDGET,
            "Corpus replay verified"
        );
        Ok(())
    });
    let closed = runtime.block_on(reader.close());
    result?;
    closed?;
    Ok(())
}

/// Fetching owns serialized reservations until ownership moves to a worker.
async fn feed(
    reader: &Arc<DbReader>,
    records: &[Record],
    sender: Sender<Ready>,
    budget: Arc<Semaphore>,
    cancellation: &CancellationToken,
) -> eyre::Result<()> {
    let mut tasks = JoinSet::new();
    let mut records = records.iter().cloned();
    let result = async {
        loop {
            eyre::ensure!(!cancellation.is_cancelled(), "corpus replay cancelled");
            while tasks.len() < 64 {
                let Some(record) = records.next() else { break };
                let sender = sender.clone();
                let reader = reader.clone();
                let budget = budget.clone();
                tasks.spawn(async move {
                    let reservation = budget.acquire_many_owned(record.bytes as u32).await?;
                    // Per-scan read-ahead belongs to SlateDB's additional memory,
                    // not the serialized-record reservation above.
                    let options = ScanOptions::default().with_read_ahead_bytes(1024 * 1024);
                    let key = store::key(record.start);
                    let mut scan = reader
                        .scan_with_options(key..store::key(record.start + 1), &options)
                        .await?;
                    let entry = scan
                        .next()
                        .await?
                        .ok_or_else(|| eyre::eyre!("missing corpus record at {}", record.start))?;
                    eyre::ensure!(
                        entry.key.as_ref() == key && entry.value.len() == record.bytes,
                        "unexpected corpus key/size"
                    );
                    eyre::ensure!(scan.next().await?.is_none(), "duplicate corpus record");
                    drop(scan);
                    let mut ready = Ready { record, bytes: entry.value, _reservation: reservation };
                    loop {
                        match sender.try_send(ready) {
                            Ok(()) => return Ok(()),
                            Err(TrySendError::Full(item)) => ready = item,
                            Err(TrySendError::Disconnected(_)) => {
                                eyre::bail!("execution workers disconnected")
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                });
            }
            if tasks.is_empty() {
                return eyre::Ok(());
            }
            tokio::select! {
                result = tasks.join_next() => { result.expect("nonempty reader tasks")??; }
                _ = tokio::time::sleep(Duration::from_millis(25)) => {}
            }
        }
    }
    .await;
    // Abort and join failed/cancelled reads before closing the pinned reader.
    tasks.shutdown().await;
    drop(sender);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::memory::InMemory;
    use slatedb::{
        config::{CheckpointOptions, CheckpointScope, DbReaderOptions},
        Db, DbReaderMode,
    };

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn slow_consumer_backpressure_cancellation_and_read_errors() -> eyre::Result<()> {
        const SIZE: usize = 128 * 1024;
        let objects = Arc::new(InMemory::new());
        let db = Db::open("feed", objects.clone()).await?;
        let payload = Bytes::from(vec![7; SIZE]);
        let records: Vec<_> = (1..=12)
            .map(|start| Record {
                start,
                end: start + 1,
                parent: [0; 32],
                tip: [0; 32],
                bytes: SIZE,
                digest: String::new(),
                transactions: 0,
                gas: 0,
            })
            .collect();
        for record in &records {
            db.put(store::key(record.start), &payload).await?;
        }
        let checkpoint =
            db.create_checkpoint(CheckpointScope::All, &CheckpointOptions::default()).await?;
        db.close().await?;
        let reader = Arc::new(
            DbReader::open(
                "feed",
                objects,
                DbReaderMode::Checkpoint(checkpoint.id),
                DbReaderOptions::default(),
            )
            .await?,
        );
        let budget = Arc::new(Semaphore::new(4 * SIZE));
        let (sender, receiver) = crossbeam_channel::bounded(4);
        let observe = receiver.clone();
        let (first_tx, first_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let consumer = tokio::task::spawn_blocking(move || {
            let first: Ready = receiver.recv().unwrap();
            first_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            let mut received = vec![first.record.start];
            assert_eq!(first.bytes, payload);
            drop(first);
            for ready in receiver {
                assert_eq!(ready.bytes, payload);
                received.push(ready.record.start);
            }
            received.sort_unstable();
            received
        });
        let producer = {
            let reader = reader.clone();
            let budget = budget.clone();
            let records = records.clone();
            tokio::spawn(async move {
                feed(&reader, &records, sender, budget, &CancellationToken::new()).await
            })
        };
        first_rx.await?;
        tokio::time::timeout(Duration::from_secs(5), async {
            while observe.len() != 3 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        // Three queued frames plus one in a consumer exhaust all four reservations.
        assert_eq!(budget.available_permits(), 0);
        assert!(!producer.is_finished());
        drop(observe);
        release_tx.send(())?;
        tokio::time::timeout(Duration::from_secs(5), producer).await???;
        assert_eq!(consumer.await?, (1..=12).collect::<Vec<_>>());
        assert_eq!(budget.available_permits(), 4 * SIZE);

        let budget = Arc::new(Semaphore::new(4 * SIZE));
        let token = CancellationToken::new();
        let (sender, receiver) = crossbeam_channel::bounded(4);
        let cancellation = {
            let token = token.clone();
            let budget = budget.clone();
            async move {
                while budget.available_permits() != 0 {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                token.cancel();
            }
        };
        let (cancelled, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(feed(&reader, &records, sender, budget.clone(), &token), cancellation)
        })
        .await?;
        assert!(cancelled.is_err());
        drop(receiver);
        assert_eq!(budget.available_permits(), 4 * SIZE);
        for missing in [false, true] {
            let (sender, receiver) = crossbeam_channel::bounded(4);
            drop(receiver);
            let mut record = records[0].clone();
            if missing {
                record.start = 999;
            }
            let result = tokio::time::timeout(
                Duration::from_secs(5),
                feed(
                    &reader,
                    &[record],
                    sender,
                    Arc::new(Semaphore::new(SIZE)),
                    &CancellationToken::new(),
                ),
            )
            .await?;
            assert!(result.is_err());
        }
        reader.close().await?;
        Ok(())
    }
}
