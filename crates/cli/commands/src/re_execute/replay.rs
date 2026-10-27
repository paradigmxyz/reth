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
    ops::Range,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    sync::{Notify, OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
};
use tracing::info;

struct Ready {
    record: Record,
    bytes: Bytes,
    cases: Vec<Bytes>,
    space: Arc<Notify>,
    // Acquired before fetching, held through decoding and execution. The Bytes
    // value moves between tasks without a framing copy or an input mutex.
    _reservation: OwnedSemaphorePermit,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run<E, C>(
    store: Store,
    catalog: Catalog,
    mut range: Range<u64>,
    config: E,
    consensus: C,
    workers: usize,
    standalone: Option<(u64, u64)>,
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
    if let Some((from, to)) = standalone {
        range = runtime.block_on(transaction_range(&reader, &catalog, from, to))?;
    }
    let budget = Arc::new(Semaphore::new(BYTE_BUDGET));
    let (sender, receiver) = crossbeam_channel::bounded::<Ready>(4);
    let started = Instant::now();
    // Save the triggering error before cancelling peers. Join order must not
    // replace missing/corrupt state with a secondary cancellation error.
    let failure = Mutex::new(None);
    let result = std::thread::scope(|scope| -> eyre::Result<()> {
        let producer = scope.spawn(|| {
            match runtime.block_on(feed_catalog(
                &reader,
                &catalog,
                range.clone(),
                standalone,
                sender,
                budget.clone(),
                &cancellation,
            )) {
                Ok(totals) => totals,
                Err(error) => {
                    failure.lock().get_or_insert(error);
                    cancellation.cancel();
                    budget.close();
                    (0, 0, 0)
                }
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
                let result = (|| -> eyre::Result<(u64, u64, u64)> {
                    let mut completed = (0, 0, 0);
                    let mut receive_wait = Duration::ZERO;
                    loop {
                        eyre::ensure!(!cancellation.is_cancelled(), "corpus worker cancelled");
                        let waiting = Instant::now();
                        let received = receiver.recv_timeout(Duration::from_millis(100));
                        receive_wait += waiting.elapsed();
                        let ready = match received {
                            Ok(ready) => ready,
                            Err(RecvTimeoutError::Timeout) => continue,
                            Err(RecvTimeoutError::Disconnected) => break,
                        };
                        ready.space.notify_one();
                        let chunk = store::decode(&ready.bytes, &ready.record)?;
                        let (transactions, gas) = if let Some((from, to)) = standalone {
                            let mut gas = 0;
                            let cases = ready
                                .cases
                                .iter()
                                .enumerate()
                                .map(|(index, bytes)| {
                                    store::decode_case(
                                        bytes,
                                        &ready.record.cases[index],
                                        ready.record.first_tx + index as u64,
                                    )
                                })
                                .collect::<eyre::Result<Vec<_>>>()?;
                            let mut transactions = 0;
                            for (index, case) in cases.iter().enumerate() {
                                eyre::ensure!(
                                    !cancellation.is_cancelled(),
                                    "case replay cancelled"
                                );
                                if case.ordinal >= from && case.ordinal < to {
                                    case.replay(config, &chunk, &cases[..index])?;
                                    gas += case.gas_used();
                                    transactions += 1;
                                }
                            }
                            eyre::ensure!(
                                ready.cases.len() as u64 == ready.record.transactions,
                                "missing standalone transactions"
                            );
                            (transactions, gas)
                        } else {
                            store::verify_cases(
                                &chunk,
                                &ready.cases,
                                &ready.record,
                                config,
                                consensus,
                                cancellation,
                            )?;
                            (ready.record.transactions, ready.record.gas)
                        };
                        completed.0 += 1;
                        completed.1 += transactions;
                        completed.2 += gas;
                        // Drop decoded state, serialized Bytes, and its permit
                        // together, before receiving more work.
                    }
                    tracing::debug!(target: "reth::corpus::feed", receive_us = receive_wait.as_micros() as u64, records = completed.0, "Worker receive intervals (summed overlap)");
                    Ok(completed)
                })();
                match result {
                    Ok(items) => items,
                    Err(error) => {
                        failure.lock().get_or_insert(error);
                        cancellation.cancel();
                        budget.close();
                        (0, 0, 0)
                    }
                }
            }));
        }
        drop(receiver);
        let mut completed = (0, 0, 0);
        for worker in threads {
            match worker.join() {
                Ok(items) => {
                    completed.0 += items.0;
                    completed.1 += items.1;
                    completed.2 += items.2;
                }
                Err(_) => {
                    failure.lock().get_or_insert_with(|| eyre::eyre!("corpus worker panicked"));
                    cancellation.cancel();
                    budget.close();
                }
            }
        }
        let wanted = producer.join().map_err(|_| eyre::eyre!("corpus reader panicked"))?;
        if let Some(error) = failure.lock().take() {
            return Err(error);
        }
        // The metadata cursor rejects gaps/duplicates before each single send;
        // MPMC receive transfers each record to exactly one worker. Counts close
        // the coverage check without retaining a history-sized completion set.
        eyre::ensure!(
            completed.0 == wanted.0 &&
                completed.1 == wanted.1 &&
                (standalone.is_some() || completed.2 == wanted.2),
            "missing/duplicate completed corpus records"
        );
        let transactions = completed.1;
        let gas = completed.2;
        let blocks = if standalone.is_some() { 0 } else { range.end - range.start };
        info!(
            blocks,
            transactions,
            gas,
            records = completed.0,
            workers,
            standalone = standalone.is_some(),
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
    let space = Arc::new(Notify::new());
    let mut records = records.iter().cloned().peekable();
    // Reserve whole read batches before opening scans, then transfer exact byte
    // permits to individual ready records. No task acquires a second reservation
    // while retaining its first, including when a record fills the entire budget.
    let batch_limit = budget.available_permits().min(8 * 1024 * 1024);
    let result = async {
        loop {
            eyre::ensure!(!cancellation.is_cancelled(), "corpus replay cancelled");
            while tasks.len() < 64 {
                let Some(record) = records.next() else { break };
                let mut bytes = record.bytes;
                let mut batch = vec![record];
                while batch.len() < 512 {
                    let Some(next) = records.peek() else { break };
                    if next.start != batch.last().expect("nonempty batch").end ||
                        bytes + next.bytes > batch_limit
                    {
                        break;
                    }
                    bytes += next.bytes;
                    batch.push(records.next().expect("peeked record"));
                }
                let sender = sender.clone();
                let reader = reader.clone();
                let budget = budget.clone();
                let space = space.clone();
                tasks.spawn(async move {
                    let waiting = Instant::now();
                    let mut reservation = budget.acquire_many_owned(bytes as u32).await?;
                    let budget_wait = waiting.elapsed();
                    let mut queue_wait = Duration::ZERO;
                    // Per-scan read-ahead belongs to SlateDB's additional memory,
                    // not the serialized-record reservation above.
                    let options = ScanOptions::default().with_read_ahead_bytes(4 * 1024 * 1024);
                    let first = batch.first().expect("nonempty batch");
                    let last = batch.last().expect("nonempty batch");
                    let tx_end = last.first_tx + last.transactions;
                    let mut block_scan = reader
                        .scan_with_options(store::key(first.start)..store::key(last.end), &options)
                        .await?;
                    let mut tx_scan = if first.first_tx == tx_end {
                        None
                    } else {
                        Some(
                            reader
                                .scan_with_options(
                                    store::transaction_key(first.first_tx)..
                                        store::transaction_key(tx_end),
                                    &options,
                                )
                                .await?,
                        )
                    };
                    for record in batch {
                        let entry = block_scan.next().await?.ok_or_else(|| {
                            eyre::eyre!("missing corpus record at {}", record.start)
                        })?;
                        eyre::ensure!(
                            entry.key.as_ref() == store::key(record.start) &&
                                entry.value.len() == record.block_bytes,
                            "unexpected corpus key/size"
                        );
                        let mut cases = Vec::with_capacity(record.cases.len());
                        for (index, expected) in record.cases.iter().enumerate() {
                            let entry = tx_scan
                                .as_mut()
                                .ok_or_else(|| eyre::eyre!("missing transaction scan"))?
                                .next()
                                .await?
                                .ok_or_else(|| eyre::eyre!("missing transaction record"))?;
                            eyre::ensure!(
                                entry.key.as_ref() ==
                                    store::transaction_key(record.first_tx + index as u64) &&
                                    entry.value.len() == expected.bytes,
                                "transaction scan gap/order/size mismatch"
                            );
                            cases.push(entry.value);
                        }
                        let permit = reservation.split(record.bytes).expect("batch sum reserved");
                        let mut ready = Ready { record, bytes: entry.value, cases, space: space.clone(), _reservation: permit };
                        loop {
                            // Register before checking capacity so a receive between
                            // try_send and await cannot leave this producer asleep.
                            let notified = space.notified();
                            tokio::pin!(notified);
                            notified.as_mut().enable();
                            match sender.try_send(ready) {
                                Ok(()) => break,
                                Err(TrySendError::Full(item)) => ready = item,
                                Err(TrySendError::Disconnected(_)) => {
                                    eyre::bail!("execution workers disconnected")
                                }
                            }
                            let waiting = Instant::now();
                            notified.await;
                            queue_wait += waiting.elapsed();
                        }
                    }
                    eyre::ensure!(block_scan.next().await?.is_none(), "extra block record");
                    if let Some(mut scan) = tx_scan {
                        eyre::ensure!(scan.next().await?.is_none(), "extra transaction record");
                    }
                    tracing::debug!(target: "reth::corpus::feed", budget_us = budget_wait.as_micros() as u64, queue_us = queue_wait.as_micros() as u64, bytes, "Producer waits (overlap across tasks)");
                    Ok(())
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

/// Locate independent cases without walking all preceding block metadata.
async fn transaction_range(
    reader: &DbReader,
    catalog: &Catalog,
    from: u64,
    to: u64,
) -> eyre::Result<Range<u64>> {
    let mut bounds = Vec::new();
    for ordinal in [from, to - 1] {
        let (mut low, mut high) = (catalog.context.start, catalog.context.end);
        while low < high {
            let mid = low + (high - low) / 2;
            let record = store::metadata(reader, mid).await?;
            let end = record
                .first_tx
                .checked_add(record.transactions)
                .ok_or_else(|| eyre::eyre!("ordinal overflow"))?;
            if end <= ordinal {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        eyre::ensure!(low < catalog.context.end, "missing transaction metadata");
        let record = store::metadata(reader, low).await?;
        eyre::ensure!(
            record.first_tx <= ordinal && ordinal < record.first_tx + record.transactions,
            "transaction ordinal metadata gap"
        );
        bounds.push(low);
    }
    Ok(bounds[0]..bounds[1] + 1)
}

async fn feed_catalog(
    reader: &Arc<DbReader>,
    catalog: &Catalog,
    range: Range<u64>,
    standalone: Option<(u64, u64)>,
    sender: Sender<Ready>,
    budget: Arc<Semaphore>,
    cancellation: &CancellationToken,
) -> eyre::Result<(u64, u64, u64)> {
    let mut cursor = if range.start == catalog.context.start {
        store::Cursor::initial(&catalog.context)?
    } else {
        let previous = store::metadata(reader, range.start - 1).await?;
        let mut cursor =
            store::Cursor { next: previous.start, tx: previous.first_tx, parent: previous.parent };
        cursor.advance(&previous)?;
        cursor
    };
    let initial_tx = cursor.tx;
    let mut totals = (0, 0, 0);
    while cursor.next < range.end {
        eyre::ensure!(!cancellation.is_cancelled(), "metadata read cancelled");
        let mut page = store::read_page(reader, &mut cursor, range.end).await?;
        if let Some((from, to)) = standalone {
            page.retain(|r| {
                r.transactions > 0 && r.first_tx < to && r.first_tx + r.transactions > from
            });
        }
        for record in &page {
            totals.0 += 1;
            totals.1 += standalone.map_or(record.transactions, |(from, to)| {
                to.min(record.first_tx + record.transactions) - from.max(record.first_tx)
            });
            totals.2 += record.gas;
        }
        feed(reader, &page, sender.clone(), budget.clone(), cancellation).await?;
    }
    if range.end == catalog.context.end {
        eyre::ensure!(cursor == catalog.cursor, "final metadata boundary mismatch");
    } else {
        let next = store::metadata(reader, range.end).await?;
        eyre::ensure!(
            cursor.parent == next.parent && cursor.tx == next.first_tx,
            "metadata boundary mismatch"
        );
    }
    eyre::ensure!(
        totals.1 == standalone.map_or(cursor.tx - initial_tx, |(from, to)| to - from),
        "metadata transaction coverage mismatch"
    );
    eyre::ensure!(
        standalone.is_some() || totals.0 == range.end - range.start,
        "metadata block coverage mismatch"
    );
    Ok(totals)
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
        // Two equal batches: whichever wins the reservation must fill the queue.
        let records: Vec<_> = (1..=16)
            .map(|start| Record {
                start,
                end: start + 1,
                parent: [0; 32],
                tip: [0; 32],
                bytes: SIZE,
                block_bytes: SIZE,
                digest: String::new(),
                transactions: 0,
                gas: 0,
                first_tx: 0,
                cases: Vec::new(),
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
        let budget = Arc::new(Semaphore::new(8 * SIZE));
        let (sender, receiver) = crossbeam_channel::bounded(4);
        let observe = receiver.clone();
        let (first_tx, first_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let consumer = tokio::task::spawn_blocking(move || {
            let first: Ready = receiver.recv().unwrap();
            first.space.notify_one();
            first_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            let mut received = vec![first.record.start];
            assert_eq!(first.bytes, payload);
            drop(first);
            for ready in receiver {
                ready.space.notify_one();
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
            while observe.len() != 4 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        // One consumer, four queued records and three reserved future/in-flight
        // records exhaust the budget. Both queue-full and byte waits must recover.
        assert_eq!(budget.available_permits(), 0);
        assert!(!producer.is_finished());
        drop(observe);
        release_tx.send(())?;
        tokio::time::timeout(Duration::from_secs(5), producer).await???;
        assert_eq!(consumer.await?, (1..=16).collect::<Vec<_>>());
        assert_eq!(budget.available_permits(), 8 * SIZE);

        let budget = Arc::new(Semaphore::new(8 * SIZE));
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
        assert_eq!(budget.available_permits(), 8 * SIZE);
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
