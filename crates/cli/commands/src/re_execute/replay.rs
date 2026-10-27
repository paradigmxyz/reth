//! Bounded in-process checkpoint reads feeding native execution workers.

use super::store::{self, Catalog, Record, Store, MAX_RECORD_BYTES};
use bytes::Bytes;
use crossbeam_channel::{RecvTimeoutError, Sender, TrySendError};
use parking_lot::Mutex;
use reth_cli_util::cancellation::CancellationToken;
use reth_consensus::FullConsensus;
use reth_evm::ConfigureEvm;
use slatedb::DbReader;
use std::{
    ops::Range,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    sync::{mpsc, Notify, OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
};
use tracing::info;

/// Serialized bytes reserved across in-flight scans, queued and executing records.
/// Each scan reserves its whole batch (at most 8 MiB) before opening, so this fits
/// ~16 maximum-size batches or two maximum-size records. Not an RSS cap: decoded
/// state, SlateDB scan/cache buffers and SDK allocations are excluded.
const REPLAY_BUDGET: usize = 2 * MAX_RECORD_BYTES;

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
    let budget = Arc::new(Semaphore::new(REPLAY_BUDGET));
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
            serialized_budget = REPLAY_BUDGET,
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
async fn feed_pages(
    reader: &Arc<DbReader>,
    mut pages: mpsc::Receiver<Vec<Record>>,
    tasks: &mut JoinSet<eyre::Result<()>>,
    sender: Sender<Ready>,
    budget: Arc<Semaphore>,
    cancellation: &CancellationToken,
) -> eyre::Result<()> {
    let space = Arc::new(Notify::new());
    let mut records = Vec::<Record>::new().into_iter().peekable();
    let mut closed = false;
    // Reserve whole read batches before opening scans, then transfer exact byte
    // permits to individual ready records. No task acquires a second reservation
    // while retaining its first, including when a record fills the entire budget.
    let batch_limit = budget.available_permits().min(8 * 1024 * 1024);
    loop {
        eyre::ensure!(!cancellation.is_cancelled(), "corpus replay cancelled");
        while tasks.len() < 64 {
            let Some(record) = records.next() else { break };
            let mut bytes = record.bytes;
            let mut batch = vec![record];
            // Batches never span pages, so an unfiltered page under the byte limit is one
            // scan task; read parallelism comes from pages in flight under the byte budget.
            while batch.len() < store::PAGE_RECORDS {
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
                let first = batch.first().expect("nonempty batch");
                let last = batch.last().expect("nonempty batch");
                let mut payloads = store::PayloadScan::open(&reader, first, last).await?;
                for record in batch {
                    let (block, cases) = payloads.next(&record).await?;
                    let permit = reservation.split(record.bytes).expect("batch sum reserved");
                    let mut ready = Ready {
                        record,
                        bytes: block,
                        cases,
                        space: space.clone(),
                        _reservation: permit,
                    };
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
                payloads.finish().await?;
                tracing::debug!(
                    target: "reth::corpus::feed",
                    budget_us = budget_wait.as_micros() as u64,
                    queue_us = queue_wait.as_micros() as u64,
                    bytes,
                    "Producer waits (overlap across tasks)"
                );
                Ok(())
            });
        }
        if closed && records.peek().is_none() && tasks.is_empty() {
            return Ok(());
        }
        tokio::select! {
            result = tasks.join_next(), if !tasks.is_empty() => {
                result.expect("nonempty reader tasks")??;
            }
            page = pages.recv(), if !closed && records.peek().is_none() && tasks.len() < 64 => {
                match page {
                    Some(page) => records = page.into_iter().peekable(),
                    None => closed = true,
                }
            }
            // The token is a polled flag; re-check it while every arm is blocked.
            _ = tokio::time::sleep(Duration::from_millis(25)) => {}
        }
    }
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
    // One pool spans the entire range, so a slow payload batch cannot stall
    // unrelated reads at a metadata page boundary.
    let mut tasks = JoinSet::new();
    let (pages, receiver) = mpsc::channel(1);
    let result = tokio::try_join!(
        read_catalog(reader, catalog, range, standalone, pages, cancellation),
        feed_pages(reader, receiver, &mut tasks, sender, budget, cancellation),
    );
    // The sibling future is dropped on error. Abort and join its payload tasks
    // before the caller closes the pinned reader, preserving the original error.
    tasks.shutdown().await;
    result.map(|(totals, ())| totals)
}

async fn read_catalog(
    reader: &Arc<DbReader>,
    catalog: &Catalog,
    range: Range<u64>,
    standalone: Option<(u64, u64)>,
    pages: mpsc::Sender<Vec<Record>>,
    cancellation: &CancellationToken,
) -> eyre::Result<(u64, u64, u64)> {
    let cursor = if range.start == catalog.context.start {
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
    let mut metadata = store::MetadataPages::open(reader, cursor, range.end).await?;
    while metadata.cursor.next < range.end {
        // Reserve before reading: only one queued/in-progress metadata page,
        // in addition to the current page and descriptors owned by bounded tasks.
        let slot = pages.reserve().await?;
        eyre::ensure!(!cancellation.is_cancelled(), "metadata read cancelled");
        let mut page = metadata.next_page().await?;
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
        // An empty filtered page is not EOF; standalone replay can skip pages.
        slot.send(page);
    }
    let cursor = metadata.cursor;
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

    async fn feed(
        reader: &Arc<DbReader>,
        records: &[Record],
        sender: Sender<Ready>,
        budget: Arc<Semaphore>,
        cancellation: &CancellationToken,
    ) -> eyre::Result<()> {
        let (pages, receiver) = mpsc::channel(1);
        pages.send(records.to_vec()).await?;
        drop(pages);
        let mut tasks = JoinSet::new();
        let result = feed_pages(reader, receiver, &mut tasks, sender, budget, cancellation).await;
        tasks.shutdown().await;
        result
    }

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

    /// Exercise the actual paged reader and payload feeder without EVM work.
    async fn paged_fixture(missing: Option<u64>) -> eyre::Result<(Arc<DbReader>, Catalog)> {
        let objects = Arc::new(InMemory::new());
        let db = Db::open("paged", objects.clone()).await?;
        let count = (2 * store::PAGE_RECORDS + 1) as u64;
        let context = store::Context {
            schema: 5,
            chain_id: 1,
            genesis: alloy_primitives::B256::ZERO.to_string(),
            forks: "test".to_owned(),
            start: 1,
            end: count + 1,
            parent: alloy_primitives::B256::ZERO.to_string(),
            tip: alloy_primitives::B256::ZERO.to_string(),
            first_tx: 0,
            end_tx: count,
        };
        for start in 1..=count {
            let record = Record {
                start,
                end: start + 1,
                parent: [0; 32],
                tip: [0; 32],
                bytes: 2,
                block_bytes: 1,
                digest: String::new(),
                transactions: 1,
                gas: 1,
                first_tx: start - 1,
                cases: vec![store::CaseRecord { index: 0, bytes: 1, digest: String::new() }],
            };
            if missing != Some(start) {
                db.put(store::metadata_key(start), serde_json::to_vec(&record)?).await?;
            }
            db.put(store::key(start), b"b").await?;
            db.put(store::transaction_key(start - 1), b"t").await?;
        }
        let checkpoint =
            db.create_checkpoint(CheckpointScope::All, &CheckpointOptions::default()).await?;
        db.close().await?;
        let reader = Arc::new(
            DbReader::open(
                "paged",
                objects,
                DbReaderMode::Checkpoint(checkpoint.id),
                DbReaderOptions::default(),
            )
            .await?,
        );
        let catalog = Catalog {
            context,
            checkpoint: Some(checkpoint.id.to_string()),
            cursor: store::Cursor { next: count + 1, tx: count, parent: [0; 32] },
            records: Vec::new(),
            complete: true,
            generation: 1,
        };
        Ok((reader, catalog))
    }

    async fn drain_catalog(
        reader: &Arc<DbReader>,
        catalog: &Catalog,
        range: Range<u64>,
        standalone: Option<(u64, u64)>,
    ) -> eyre::Result<((u64, u64, u64), Vec<u64>)> {
        let budget = Arc::new(Semaphore::new(REPLAY_BUDGET));
        let (sender, receiver) = crossbeam_channel::bounded::<Ready>(4);
        let consumer = tokio::task::spawn_blocking(move || {
            let mut received = Vec::new();
            for ready in receiver {
                ready.space.notify_one();
                assert_eq!(ready.bytes.as_ref(), b"b");
                assert_eq!(ready.cases, vec![Bytes::from_static(b"t")]);
                received.push(ready.record.start);
            }
            received.sort_unstable();
            received
        });
        let result = feed_catalog(
            reader,
            catalog,
            range,
            standalone,
            sender,
            budget.clone(),
            &CancellationToken::new(),
        )
        .await;
        // Join the consumer on both success and error before inspecting permits.
        let received = consumer.await?;
        assert_eq!(budget.available_permits(), REPLAY_BUDGET);
        Ok((result?, received))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn paged_catalog_ranges_and_failures() -> eyre::Result<()> {
        tokio::time::timeout(Duration::from_secs(30), async {
            let (reader, catalog) = paged_fixture(None).await?;
            for (range, standalone) in [
                (1..catalog.context.end, None),
                (1..2, None),
                (999..2001, None),
                (999..2001, Some((999, 1999))),
                (1..catalog.context.end, Some((2000, 2001))),
            ] {
                let (totals, received) =
                    drain_catalog(&reader, &catalog, range.clone(), standalone).await?;
                let expected: Vec<_> = range
                    .filter(|start| {
                        standalone.is_none_or(|(from, to)| *start > from && *start <= to)
                    })
                    .collect();
                let count = expected.len() as u64;
                assert_eq!(totals, (count, count, count));
                assert_eq!(received, expected);
            }
            let mut corrupt = catalog.clone();
            corrupt.cursor.parent = [1; 32];
            let error =
                drain_catalog(&reader, &corrupt, 1..catalog.context.end, None).await.unwrap_err();
            assert_eq!(error.to_string(), "final metadata boundary mismatch");
            reader.close().await?;

            let (reader, catalog) = paged_fixture(Some(store::PAGE_RECORDS as u64 + 1)).await?;
            let error =
                drain_catalog(&reader, &catalog, 1..catalog.context.end, None).await.unwrap_err();
            assert_eq!(error.to_string(), "metadata scan key/size mismatch");
            reader.close().await?;
            eyre::Ok(())
        })
        .await?
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn paged_catalog_cancellation_releases_reservations() -> eyre::Result<()> {
        let (reader, catalog) = paged_fixture(None).await?;
        let budget = Arc::new(Semaphore::new(8));
        let token = CancellationToken::new();
        let (sender, receiver) = crossbeam_channel::bounded::<Ready>(4);
        let cancel = async {
            while receiver.len() < 4 {
                tokio::task::yield_now().await;
            }
            token.cancel();
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(
                feed_catalog(
                    &reader,
                    &catalog,
                    1..catalog.context.end,
                    None,
                    sender,
                    budget.clone(),
                    &token
                ),
                cancel,
            )
        })
        .await?;
        // Either sibling may observe cancellation first.
        assert!(matches!(
            result.unwrap_err().to_string().as_str(),
            "corpus replay cancelled" | "metadata read cancelled"
        ));
        drop(receiver);
        assert_eq!(budget.available_permits(), 8);
        reader.close().await?;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn metadata_error_releases_payload_reservations() -> eyre::Result<()> {
        let (reader, catalog) = paged_fixture(Some(store::PAGE_RECORDS as u64 + 1)).await?;
        let budget = Arc::new(Semaphore::new(128));
        let (sender, receiver) = crossbeam_channel::bounded::<Ready>(4);
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            feed_catalog(
                &reader,
                &catalog,
                1..catalog.context.end,
                None,
                sender,
                budget.clone(),
                &CancellationToken::new(),
            ),
        )
        .await?;
        assert_eq!(result.unwrap_err().to_string(), "metadata scan key/size mismatch");
        drop(receiver);
        assert_eq!(budget.available_permits(), 128);
        reader.close().await?;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn next_page_does_not_wait_for_older_payload_tasks() -> eyre::Result<()> {
        let (reader, catalog) = paged_fixture(None).await?;
        let budget = Arc::new(Semaphore::new(REPLAY_BUDGET));
        let token = CancellationToken::new();
        let (sender, receiver) = crossbeam_channel::bounded::<Ready>(4);
        let (release, gate) = tokio::sync::oneshot::channel();
        let mut tasks = JoinSet::new();
        // Model a lingering payload task from an earlier page. Page two must
        // arrive before it can finish, not merely overlap metadata fetching.
        tasks.spawn(async move {
            gate.await?;
            Ok(())
        });
        let consumer = tokio::task::spawn_blocking(move || {
            let mut release = Some(release);
            let mut seen = Vec::new();
            for ready in receiver {
                ready.space.notify_one();
                if ready.record.start > store::PAGE_RECORDS as u64 &&
                    let Some(release) = release.take()
                {
                    let _ = release.send(());
                }
                seen.push(ready.record.start);
            }
            seen.sort_unstable();
            seen
        });
        let (pages, receiver) = mpsc::channel(1);
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::try_join!(
                read_catalog(&reader, &catalog, 1..catalog.context.end, None, pages, &token),
                feed_pages(&reader, receiver, &mut tasks, sender, budget.clone(), &token),
            )
        })
        .await;
        tasks.shutdown().await;
        let seen = consumer.await?;
        assert_eq!(result??.0, (2001, 2001, 2001));
        assert_eq!(seen, (1..catalog.context.end).collect::<Vec<_>>());
        assert_eq!(budget.available_permits(), REPLAY_BUDGET);
        reader.close().await?;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn full_reader_pool_leaves_next_page_queued() -> eyre::Result<()> {
        let (reader, _) = paged_fixture(None).await?;
        let budget = Arc::new(Semaphore::new(REPLAY_BUDGET));
        let token = CancellationToken::new();
        let (sender, receiver) = crossbeam_channel::bounded::<Ready>(4);
        let (pages, input) = mpsc::channel(1);
        pages.send(vec![store::metadata(&reader, 1).await?]).await?;
        let mut tasks = JoinSet::new();
        for _ in 0..64 {
            tasks.spawn(std::future::pending::<eyre::Result<()>>());
        }
        let mut feeding =
            Box::pin(feed_pages(&reader, input, &mut tasks, sender, budget.clone(), &token));
        // Poll once with a full pool. A 65th task must not be admitted, and the
        // next metadata page must stay queued rather than grow a pending list.
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(feeding.as_mut(), cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        assert_eq!(pages.capacity(), 0);
        assert!(receiver.is_empty());
        drop(feeding);
        assert_eq!(tasks.len(), 64);
        tasks.shutdown().await;
        assert_eq!(budget.available_permits(), REPLAY_BUDGET);
        reader.close().await?;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_while_waiting_for_first_page() -> eyre::Result<()> {
        let (reader, _) = paged_fixture(None).await?;
        let budget = Arc::new(Semaphore::new(REPLAY_BUDGET));
        let token = CancellationToken::new();
        let (sender, receiver) = crossbeam_channel::bounded::<Ready>(4);
        let (pages, input) = mpsc::channel(1);
        let mut tasks = JoinSet::new();
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(
                feed_pages(&reader, input, &mut tasks, sender, budget.clone(), &token),
                async {
                    tokio::task::yield_now().await;
                    token.cancel();
                },
            )
            .0
        })
        .await;
        tasks.shutdown().await;
        assert_eq!(result?.unwrap_err().to_string(), "corpus replay cancelled");
        assert!(receiver.is_empty());
        assert_eq!(budget.available_permits(), REPLAY_BUDGET);
        drop(pages);
        reader.close().await?;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "manual feeder microbenchmark; excludes EVM and remote object storage"]
    async fn paged_catalog_feed_benchmark() -> eyre::Result<()> {
        let (reader, catalog) = paged_fixture(None).await?;
        for iteration in 0..12 {
            let started = Instant::now();
            let (totals, _) =
                drain_catalog(&reader, &catalog, 1..catalog.context.end, None).await?;
            assert_eq!(totals, (2001, 2001, 2001));
            eprintln!(
                "feed_bench iteration={iteration} seconds={:.6}",
                started.elapsed().as_secs_f64()
            );
        }
        reader.close().await?;
        Ok(())
    }
}
