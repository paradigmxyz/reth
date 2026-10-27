//! Pinned corpus publication and read-only checkpoint access.

use async_trait::async_trait;
use aws_credential_types::provider::{ProvideCredentials, SharedCredentialsProvider};
use bytes::Bytes;
use futures::TryStreamExt;
use object_store::{
    aws::{AmazonS3Builder, AmazonS3ConfigKey, AwsCredential},
    path::Path,
    ClientConfigKey, CredentialProvider, ObjectStore, ObjectStoreExt, PutMode, PutOptions,
    RetryConfig, UpdateVersion,
};
use serde::{Deserialize, Serialize};
use slatedb::{
    config::{CheckpointOptions, CheckpointScope, DbReaderOptions, FlushOptions, FlushType},
    Db, DbReader, DbReaderMode,
};
use std::{sync::Arc, time::Duration};
use url::Url;

/// Maximum serialized size of one block record (block plus its transactions) or one
/// metadata entry. Capture rejects larger records; readers reject larger entries.
pub(super) const MAX_RECORD_BYTES: usize = 64 * 1024 * 1024;
const MAX_CATALOG_BYTES: usize = 1024 * 1024;
pub(super) const PAGE_RECORDS: usize = 1000;
/// A stuck object-store request must fail, not stall capture or replay for minutes.
/// object_store owns retries: each attempt is cut at REQUEST_TIMEOUT and no retry
/// starts after RETRY_TIMEOUT. The defaults (30 s, up to 10 retries within 3 min)
/// under SlateDB's own retries let one stuck request stall a page for 5+ minutes.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const RETRY_TIMEOUT: Duration = Duration::from_secs(60);
/// SlateDB's wrapper retries on top of object_store; disabled so they don't compound.
pub(super) const SLATEDB_RETRIES: Option<u32> = Some(0);
pub(super) const CONTEXT_KEY: &[u8] = b"reth-corpus-context";
pub(super) const CURSOR_KEY: &[u8] = b"reth-corpus-cursor";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Context {
    pub schema: u32,
    pub chain_id: u64,
    pub genesis: String,
    pub forks: String,
    pub start: u64,
    pub end: u64,
    pub parent: String,
    pub tip: String,
    pub first_tx: u64,
    pub end_tx: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Record {
    pub start: u64,
    pub end: u64,
    pub parent: [u8; 32],
    pub tip: [u8; 32],
    /// Total serialized block and transaction bytes reserved together before fetching.
    pub bytes: usize,
    pub block_bytes: usize,
    pub digest: String,
    pub transactions: u64,
    pub gas: u64,
    pub first_tx: u64,
    pub cases: Vec<CaseRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct CaseRecord {
    pub index: u64,
    pub bytes: usize,
    pub digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Catalog {
    pub context: Context,
    pub checkpoint: Option<String>,
    pub cursor: Cursor,
    /// Only the unpublished bounded page lives in memory, never in root JSON.
    #[serde(skip)]
    pub records: Vec<Record>,
    pub complete: bool,
    /// Changed by conditional PUT before opening a writer. A superseded writer
    /// never rereads/retries its publication token after a failed CAS.
    pub generation: u64,
}

impl Catalog {
    pub fn validate(&self) -> eyre::Result<()> {
        let c = &self.context;
        eyre::ensure!(
            c.schema == 5 && c.start > 0 && c.end > c.start && c.end_tx >= c.first_tx,
            "unsupported corpus context/range"
        );
        eyre::ensure!(
            self.cursor.next >= c.start &&
                self.cursor.next <= c.end &&
                self.cursor.tx >= c.first_tx &&
                self.cursor.tx <= c.end_tx,
            "invalid published cursor"
        );
        if self.cursor.next == c.start {
            eyre::ensure!(self.cursor == Cursor::initial(c)?, "invalid initial cursor");
        }
        let mut cursor = self.cursor.clone();
        for record in &self.records {
            cursor.advance(record)?;
        }
        eyre::ensure!(cursor.next <= c.end && cursor.tx <= c.end_tx, "extra records/ordinals");
        eyre::ensure!(
            !self.complete ||
                (cursor.next == c.end &&
                    cursor.tx == c.end_tx &&
                    cursor.parent == c.tip.parse::<alloy_primitives::B256>()?.0),
            "incomplete corpus publication"
        );
        eyre::ensure!(
            (self.cursor.next == c.start && self.records.is_empty()) || self.checkpoint.is_some(),
            "missing pinned checkpoint"
        );
        Ok(())
    }

    /// Advance the durable root only after the page and its metadata are verified.
    pub fn seal(&mut self) -> eyre::Result<()> {
        for record in &self.records {
            self.cursor.advance(record)?;
        }
        self.records.clear();
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(super) struct Store {
    pub objects: Arc<dyn ObjectStore>,
    pub path: Path,
}

#[derive(Debug)]
struct Credentials(SharedCredentialsProvider);

#[async_trait]
impl CredentialProvider for Credentials {
    type Credential = AwsCredential;

    async fn get_credential(&self) -> object_store::Result<Arc<AwsCredential>> {
        let credential = self.0.provide_credentials().await.map_err(|source| {
            object_store::Error::Generic { store: "AWS credential chain", source: Box::new(source) }
        })?;
        Ok(Arc::new(AwsCredential {
            key_id: credential.access_key_id().to_owned(),
            secret_key: credential.secret_access_key().to_owned(),
            token: credential.session_token().map(str::to_owned),
        }))
    }
}

/// Bounds every request without replacing other client options read from the environment.
fn bounded(builder: AmazonS3Builder, request: Duration, retry: Duration) -> AmazonS3Builder {
    let millis = |d: Duration| format!("{}ms", d.as_millis());
    builder
        .with_config(AmazonS3ConfigKey::Client(ClientConfigKey::Timeout), millis(request))
        .with_config(
            AmazonS3ConfigKey::Client(ClientConfigKey::ConnectTimeout),
            millis(CONNECT_TIMEOUT.min(request)),
        )
        .with_retry(RetryConfig { max_retries: 5, retry_timeout: retry, ..Default::default() })
}

impl Store {
    pub async fn open(location: &str, endpoint: Option<&str>) -> eyre::Result<Self> {
        let url = Url::parse(location)?;
        eyre::ensure!(
            url.username().is_empty() &&
                url.password().is_none() &&
                url.query().is_none() &&
                url.fragment().is_none(),
            "credentials/query/fragment not allowed in corpus URL"
        );
        match url.scheme() {
            "file" => {
                eyre::ensure!(endpoint.is_none(), "local corpus cannot use an S3 endpoint");
                let directory =
                    url.to_file_path().map_err(|_| eyre::eyre!("invalid local corpus path"))?;
                Ok(Self {
                    objects: Arc::new(object_store::local::LocalFileSystem::new_with_prefix(
                        directory,
                    )?),
                    path: Path::from("corpus-v5"),
                })
            }
            "s3" => {
                let bucket = url.host_str().ok_or_else(|| eyre::eyre!("missing S3 bucket"))?;
                let prefix = url.path().trim_matches('/');
                eyre::ensure!(!prefix.is_empty(), "a non-root corpus prefix is required");
                let sdk = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
                let credentials = sdk
                    .credentials_provider()
                    .ok_or_else(|| eyre::eyre!("AWS credential provider unavailable"))?;
                let mut builder = bounded(
                    AmazonS3Builder::from_env()
                        .with_bucket_name(bucket)
                        .with_region(sdk.region().map(|r| r.as_ref()).unwrap_or("us-east-1"))
                        .with_credentials(Arc::new(Credentials(credentials))),
                    REQUEST_TIMEOUT,
                    RETRY_TIMEOUT,
                );
                if let Some(endpoint) = endpoint {
                    let endpoint_url = Url::parse(endpoint)?;
                    eyre::ensure!(
                        endpoint_url.username().is_empty() &&
                            endpoint_url.password().is_none() &&
                            endpoint_url.query().is_none(),
                        "credentials are not allowed in S3 endpoint"
                    );
                    builder = builder
                        .with_endpoint(endpoint)
                        .with_allow_http(endpoint_url.scheme() == "http");
                }
                Ok(Self {
                    objects: Arc::new(builder.build()?),
                    path: Path::from(prefix).join("corpus-v5"),
                })
            }
            _ => eyre::bail!("corpus must use s3://bucket/prefix or file:///directory"),
        }
    }

    pub async fn catalog(&self) -> eyre::Result<Option<(Catalog, UpdateVersion)>> {
        let result = match self.objects.get(&self.path.clone().join("catalog.json")).await {
            Ok(result) => result,
            Err(object_store::Error::NotFound { .. }) => {
                eyre::ensure!(
                    self.objects.list(Some(&self.path)).try_next().await?.is_none(),
                    "nonempty corpus prefix has no catalog; refusing to overwrite unknown data"
                );
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        eyre::ensure!(result.meta.size <= MAX_CATALOG_BYTES as u64, "catalog exceeds size limit");
        let version = UpdateVersion {
            e_tag: result.meta.e_tag.clone(),
            version: result.meta.version.clone(),
        };
        let catalog: Catalog = serde_json::from_slice(&result.bytes().await?)?;
        catalog.validate()?;
        Ok(Some((catalog, version)))
    }

    pub async fn publish(
        &self,
        catalog: &Catalog,
        prior: Option<UpdateVersion>,
    ) -> eyre::Result<UpdateVersion> {
        catalog.validate()?;
        eyre::ensure!(catalog.records.is_empty(), "unsealed metadata page");
        let bytes = serde_json::to_vec(catalog)?;
        eyre::ensure!(bytes.len() <= MAX_CATALOG_BYTES, "catalog exceeds size limit");
        let result = self
            .objects
            .put_opts(
                &self.path.clone().join("catalog.json"),
                Bytes::from(bytes).into(),
                PutOptions {
                    mode: prior.map(PutMode::Update).unwrap_or(PutMode::Create),
                    ..Default::default()
                },
            )
            .await?;
        Ok(UpdateVersion { e_tag: result.e_tag, version: result.version })
    }

    pub async fn reader(&self, catalog: &Catalog) -> eyre::Result<DbReader> {
        let checkpoint = catalog
            .checkpoint
            .as_ref()
            .ok_or_else(|| eyre::eyre!("corpus has no published checkpoint"))?
            .parse()?;
        let reader = DbReader::open(
            self.path.clone().join("data"),
            self.objects.clone(),
            DbReaderMode::Checkpoint(checkpoint),
            DbReaderOptions { object_store_max_retries: SLATEDB_RETRIES, ..Default::default() },
        )
        .await?;
        let checked = async {
            let bytes = reader
                .get(CONTEXT_KEY)
                .await?
                .ok_or_else(|| eyre::eyre!("checkpoint has no corpus context"))?;
            let context: Context = serde_json::from_slice(&bytes)?;
            eyre::ensure!(context == catalog.context, "checkpoint/catalog context mismatch");
            let cursor = reader
                .get(CURSOR_KEY)
                .await?
                .ok_or_else(|| eyre::eyre!("checkpoint has no publication cursor"))?;
            let cursor: Cursor = serde_json::from_slice(&cursor)?;
            eyre::ensure!(cursor == catalog.cursor, "checkpoint/catalog cursor mismatch");
            eyre::Ok(())
        }
        .await;
        if let Err(error) = checked {
            reader.close().await?;
            return Err(error);
        }
        Ok(reader)
    }

    pub async fn checkpoint(&self, db: &Db) -> eyre::Result<String> {
        db.flush_with_options(FlushOptions { flush_type: FlushType::MemTable }).await?;
        Ok(db
            .create_checkpoint(CheckpointScope::All, &CheckpointOptions::default())
            .await?
            .id
            .to_string())
    }
}

pub(super) fn key(start: u64) -> [u8; 9] {
    let mut key = [b'b'; 9];
    key[1..].copy_from_slice(&start.to_be_bytes());
    key
}

pub(super) fn transaction_key(ordinal: u64) -> [u8; 9] {
    let mut key = key(ordinal);
    key[0] = b't';
    key
}

pub(super) fn decode(bytes: &[u8], record: &Record) -> eyre::Result<Arc<super::chunk::Chunk>> {
    eyre::ensure!(
        bytes.len() == record.block_bytes && blake3::hash(bytes).to_hex().as_str() == record.digest,
        "corpus record integrity mismatch"
    );
    let chunk: super::chunk::Chunk = wincode::deserialize_exact(bytes)?;
    chunk.validate()?;
    eyre::ensure!(
        chunk.start == record.start &&
            chunk.end == record.end &&
            chunk.parent == record.parent &&
            chunk.blocks.last().map(|b| b.hash) == Some(record.tip),
        "corpus record/context mismatch"
    );
    Ok(Arc::new(chunk))
}

pub(super) fn decode_case(
    bytes: &[u8],
    expected: &CaseRecord,
    ordinal: u64,
) -> eyre::Result<super::transaction::Transaction> {
    eyre::ensure!(
        bytes.len() == expected.bytes && blake3::hash(bytes).to_hex().as_str() == expected.digest,
        "transaction record integrity mismatch"
    );
    let case: super::transaction::Transaction = wincode::deserialize_exact(bytes)?;
    eyre::ensure!(
        case.ordinal == ordinal && case.index == expected.index,
        "transaction ordinal/index/key mismatch"
    );
    Ok(case)
}

/// Streams each record's block and transaction payloads with one block scan and one
/// transaction scan per contiguous page, instead of a get plus a scan per block.
pub(super) async fn for_each_payload(
    reader: &DbReader,
    records: &[Record],
    mut f: impl FnMut(&Record, Bytes, Vec<Bytes>) -> eyre::Result<()>,
) -> eyre::Result<()> {
    let (Some(first), Some(last)) = (records.first(), records.last()) else { return Ok(()) };
    let tx_end = last
        .first_tx
        .checked_add(last.transactions)
        .ok_or_else(|| eyre::eyre!("transaction ordinal overflow"))?;
    let options = slatedb::config::ScanOptions::default().with_read_ahead_bytes(1024 * 1024);
    let mut blocks = reader.scan_with_options(key(first.start)..key(last.end), &options).await?;
    let mut transactions = reader
        .scan_with_options(transaction_key(first.first_tx)..transaction_key(tx_end), &options)
        .await?;
    for record in records {
        eyre::ensure!(
            record.cases.len() as u64 == record.transactions,
            "transaction descriptor count mismatch"
        );
        let block = blocks.next().await?.ok_or_else(|| eyre::eyre!("missing published record"))?;
        eyre::ensure!(block.key.as_ref() == key(record.start), "block scan gap/order mismatch");
        let mut cases = Vec::with_capacity(record.cases.len());
        for (index, expected) in record.cases.iter().enumerate() {
            let entry = transactions
                .next()
                .await?
                .ok_or_else(|| eyre::eyre!("missing transaction record"))?;
            eyre::ensure!(
                entry.key.as_ref() == transaction_key(record.first_tx + index as u64),
                "transaction scan gap/order mismatch"
            );
            eyre::ensure!(
                entry.value.len() == expected.bytes &&
                    blake3::hash(&entry.value).to_hex().as_str() == expected.digest,
                "transaction record integrity mismatch"
            );
            cases.push(entry.value);
        }
        f(record, block.value, cases)?;
    }
    eyre::ensure!(blocks.next().await?.is_none(), "extra block record");
    eyre::ensure!(transactions.next().await?.is_none(), "extra transaction record");
    Ok(())
}

/// All phase comparisons, consensus and history checks share one actual execution.
pub(super) fn verify_cases<
    E: reth_evm::ConfigureEvm,
    C: reth_consensus::FullConsensus<E::Primitives> + ?Sized,
>(
    chunk: &Arc<super::chunk::Chunk>,
    cases: &[Bytes],
    record: &Record,
    config: &E,
    consensus: &C,
    cancellation: &reth_cli_util::cancellation::CancellationToken,
) -> eyre::Result<()> {
    eyre::ensure!(cases.len() == record.cases.len(), "transaction count mismatch");
    let expected = cases
        .iter()
        .enumerate()
        .map(|(i, b)| decode_case(b, &record.cases[i], record.first_tx + i as u64))
        .collect::<eyre::Result<Vec<_>>>()?;
    let (actual, _) = super::transaction::execute(
        config,
        consensus,
        chunk,
        record.first_tx,
        Some(&expected),
        cancellation,
    )?;
    eyre::ensure!(
        actual.len() as u64 == record.transactions &&
            actual.iter().map(|c| c.gas_used()).sum::<u64>() == record.gas,
        "transaction/gas totals mismatch"
    );
    Ok(())
}

/// Durable boundary after the last completely captured block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Cursor {
    pub next: u64,
    pub tx: u64,
    pub parent: [u8; 32],
}

impl Cursor {
    pub fn initial(context: &Context) -> eyre::Result<Self> {
        Ok(Self {
            next: context.start,
            tx: context.first_tx,
            parent: context.parent.parse::<alloy_primitives::B256>()?.0,
        })
    }

    pub fn advance(&mut self, record: &Record) -> eyre::Result<()> {
        eyre::ensure!(
            record.start == self.next &&
                record.end ==
                    record
                        .start
                        .checked_add(1)
                        .ok_or_else(|| eyre::eyre!("block overflow"))?,
            "catalog block gap/duplicate"
        );
        eyre::ensure!(
            record.parent == self.parent && record.bytes > 0 && record.bytes <= MAX_RECORD_BYTES,
            "catalog hash chain or record size mismatch"
        );
        let bytes =
            record.cases.iter().enumerate().try_fold(record.block_bytes, |sum, (i, case)| {
                eyre::ensure!(
                    case.index == i as u64 && case.bytes > 0 && case.bytes <= MAX_RECORD_BYTES,
                    "transaction index/size mismatch"
                );
                sum.checked_add(case.bytes).ok_or_else(|| eyre::eyre!("record size overflow"))
            })?;
        eyre::ensure!(
            record.block_bytes > 0 &&
                bytes == record.bytes &&
                record.first_tx == self.tx &&
                record.transactions == record.cases.len() as u64,
            "transaction coverage/size mismatch"
        );
        self.tx = self
            .tx
            .checked_add(record.transactions)
            .ok_or_else(|| eyre::eyre!("ordinal overflow"))?;
        self.next = record.end;
        self.parent = record.tip;
        Ok(())
    }
}

pub(super) fn metadata_key(block: u64) -> [u8; 9] {
    let mut key = key(block);
    key[0] = b'm';
    key
}

pub(super) async fn metadata(reader: &DbReader, block: u64) -> eyre::Result<Record> {
    let bytes = reader
        .get(metadata_key(block))
        .await?
        .ok_or_else(|| eyre::eyre!("missing block metadata at {block}"))?;
    eyre::ensure!(bytes.len() <= MAX_RECORD_BYTES, "metadata record too large");
    let record: Record = serde_json::from_slice(&bytes)?;
    eyre::ensure!(
        record.start == block &&
            record.end == block.checked_add(1).ok_or_else(|| eyre::eyre!("block overflow"))?,
        "metadata key/range mismatch"
    );
    Ok(record)
}

/// One metadata scan over a whole range, read in bounded pages. Reopening a scan
/// per page re-seeks the overlapping SSTs and serializes a fetch per page.
/// Bounded metadata pages share the data checkpoint; no growing JSON list or separate publication.
pub(super) struct MetadataPages {
    scan: slatedb::DbIterator,
    pub cursor: Cursor,
    end: u64,
}

impl MetadataPages {
    pub(super) async fn open(reader: &DbReader, cursor: Cursor, end: u64) -> eyre::Result<Self> {
        let options = slatedb::config::ScanOptions::default().with_read_ahead_bytes(1024 * 1024);
        let scan = reader
            .scan_with_options(metadata_key(cursor.next)..metadata_key(end), &options)
            .await?;
        Ok(Self { scan, cursor, end })
    }

    pub(super) async fn next_page(&mut self) -> eyre::Result<Vec<Record>> {
        let (cursor, end) = (&mut self.cursor, self.end);
        let mut records = Vec::new();
        let mut bytes = 0;
        while cursor.next < end && records.len() < PAGE_RECORDS && bytes < 8 * 1024 * 1024 {
            let entry = self
                .scan
                .next()
                .await?
                .ok_or_else(|| eyre::eyre!("metadata gap at {}", cursor.next))?;
            eyre::ensure!(
                entry.key.as_ref() == metadata_key(cursor.next) &&
                    entry.value.len() <= MAX_RECORD_BYTES,
                "metadata scan key/size mismatch"
            );
            bytes += entry.value.len();
            let record = serde_json::from_slice(&entry.value)?;
            cursor.advance(&record)?;
            records.push(record);
        }
        if cursor.next == end {
            eyre::ensure!(self.scan.next().await?.is_none(), "extra metadata record");
        }
        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use object_store::memory::InMemory;
    use proptest::prelude::*;

    fn catalog(start: u64, count: usize, transactions: u64) -> Catalog {
        let records: Vec<_> = (0..count)
            .map(|i| Record {
                start: start + i as u64,
                end: start + i as u64 + 1,
                parent: [i as u8; 32],
                tip: [(i as u8).wrapping_add(1); 32],
                bytes: 1 + transactions as usize,
                block_bytes: 1,
                digest: blake3::hash(b"a").to_hex().to_string(),
                transactions,
                gas: 0,
                first_tx: i as u64 * transactions,
                cases: (0..transactions)
                    .map(|index| CaseRecord {
                        index,
                        bytes: 1,
                        digest: blake3::hash(b"a").to_hex().to_string(),
                    })
                    .collect(),
            })
            .collect();
        Catalog {
            cursor: Cursor { next: start, tx: 0, parent: [0; 32] },
            context: Context {
                schema: 5,
                chain_id: 1,
                genesis: B256::ZERO.to_string(),
                forks: "test".to_owned(),
                start,
                end: start + count as u64,
                parent: B256::ZERO.to_string(),
                tip: B256::from([count as u8; 32]).to_string(),
                first_tx: 0,
                end_tx: count as u64 * transactions,
            },
            records,
            checkpoint: Some("00000000-0000-0000-0000-000000000000".to_owned()),
            complete: true,
            generation: 1,
        }
    }

    proptest! {
        #[test]
        fn coverage_rejects_block_and_ordinal_gaps(start in 1u64..1_000_000, count in 2usize..64, size in 0u64..100) {
            let expected = catalog(start, count, size);
            prop_assert!(expected.validate().is_ok());
            let mut sealed = expected.clone();
            sealed.seal().unwrap();
            let encoded = serde_json::to_vec(&sealed).unwrap();
            let roundtrip: Catalog = serde_json::from_slice(&encoded).unwrap();
            prop_assert!(roundtrip.validate().is_ok());
            prop_assert!(roundtrip.records.is_empty());
            let mut missing = expected.clone();
            missing.records.remove(count/2);
            prop_assert!(missing.validate().is_err());
            let mut extra = expected.clone();
            extra.records.push(extra.records.last().unwrap().clone());
            prop_assert!(extra.validate().is_err());
            let mut ordinal_gap = expected.clone();
            ordinal_gap.records[1].first_tx += 1;
            prop_assert!(ordinal_gap.validate().is_err());
            let mut missing_case = expected.clone();
            missing_case.records[0].transactions += 1;
            prop_assert!(missing_case.validate().is_err());
            let mut reordered = expected;
            reordered.records.swap(0, 1);
            prop_assert!(reordered.validate().is_err());
        }

        #[test]
        fn keys_sort_by_global_ordinal_not_decimal_text(a in any::<u64>(), b in any::<u64>()) {
            prop_assert_eq!(transaction_key(a).cmp(&transaction_key(b)), a.cmp(&b));
            prop_assert_ne!(transaction_key(a), key(a));
        }
    }

    #[tokio::test]
    async fn publication_is_fenced_and_readers_remain_pinned() -> eyre::Result<()> {
        let store = Store { objects: Arc::new(InMemory::new()), path: Path::from("test") };
        let mut catalog = catalog(1, 1, 1);
        catalog.records.clear();
        catalog.complete = false;
        catalog.checkpoint = None;
        let first = store.publish(&catalog, None).await?;
        catalog.generation += 1;
        let second = store.publish(&catalog, Some(first.clone())).await?;
        assert!(store.publish(&catalog, Some(first)).await.is_err());
        let db = Db::open(store.path.clone().join("data"), store.objects.clone()).await?;
        db.put(CONTEXT_KEY, serde_json::to_vec(&catalog.context)?).await?;
        db.put(CURSOR_KEY, serde_json::to_vec(&catalog.cursor)?).await?;
        db.put(key(1), b"first").await?;
        catalog.checkpoint = Some(store.checkpoint(&db).await?);
        store.publish(&catalog, Some(second)).await?;
        db.put(key(1), b"second").await?;
        db.flush().await?;
        db.close().await?;
        let objects_before: Vec<_> = store.objects.list(None).try_collect().await?;
        let reader = store.reader(&catalog).await?;
        assert_eq!(reader.get(key(1)).await?, Some(Bytes::from_static(b"first")));
        reader.close().await?;
        let objects_after: Vec<_> = store.objects.list(None).try_collect().await?;
        assert_eq!(objects_before, objects_after);
        Ok(())
    }

    #[tokio::test]
    async fn stuck_endpoint_fails_within_bounds() -> eyre::Result<()> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        // Accept connections and never answer, like a hung object-store request.
        let held = tokio::spawn(async move {
            let mut open = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                open.push(socket);
            }
        });
        let objects = bounded(
            AmazonS3Builder::new()
                .with_bucket_name("corpus")
                .with_region("us-east-1")
                .with_access_key_id("key")
                .with_secret_access_key("secret")
                .with_endpoint(format!("http://{address}"))
                .with_allow_http(true),
            Duration::from_millis(200),
            Duration::from_secs(1),
        )
        .build()?;
        let started = std::time::Instant::now();
        assert!(objects.get(&Path::from("catalog.json")).await.is_err());
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
        held.abort();
        Ok(())
    }

    #[tokio::test]
    async fn batched_payloads_match_records_and_reject_gaps() -> eyre::Result<()> {
        let objects = Arc::new(InMemory::new());
        let db = Db::open("payloads", objects.clone()).await?;
        let mut records = Vec::new();
        let mut tx = 0;
        for (start, count) in [(1u64, 2u64), (2, 0), (3, 1)] {
            let mut cases = Vec::new();
            for index in 0..count {
                let value = format!("t{}", tx + index).into_bytes();
                cases.push(CaseRecord {
                    index,
                    bytes: value.len(),
                    digest: blake3::hash(&value).to_hex().to_string(),
                });
                db.put(transaction_key(tx + index), &value).await?;
            }
            db.put(key(start), format!("b{start}").as_bytes()).await?;
            records.push(Record {
                start,
                end: start + 1,
                parent: [0; 32],
                tip: [0; 32],
                bytes: 1,
                block_bytes: 1,
                digest: String::new(),
                transactions: count,
                gas: 0,
                first_tx: tx,
                cases,
            });
            tx += count;
        }
        async fn open(db: &Db, objects: Arc<InMemory>) -> eyre::Result<DbReader> {
            let id =
                db.create_checkpoint(CheckpointScope::All, &CheckpointOptions::default()).await?.id;
            let mode = DbReaderMode::Checkpoint(id);
            Ok(DbReader::open("payloads", objects, mode, DbReaderOptions::default()).await?)
        }
        let reader = open(&db, objects.clone()).await?;
        let mut seen = Vec::new();
        for_each_payload(&reader, &records, |record, block, cases| {
            seen.push((record.start, block, cases));
            Ok(())
        })
        .await?;
        let expected: Vec<(u64, Bytes, Vec<Bytes>)> = vec![
            (1, "b1".into(), vec!["t0".into(), "t1".into()]),
            (2, "b2".into(), vec![]),
            (3, "b3".into(), vec!["t2".into()]),
        ];
        assert_eq!(seen, expected);
        reader.close().await?;

        db.put(transaction_key(1), b"tampered").await?;
        let reader = open(&db, objects.clone()).await?;
        let error = for_each_payload(&reader, &records, |_, _, _| Ok(())).await.unwrap_err();
        assert_eq!(error.to_string(), "transaction record integrity mismatch");
        reader.close().await?;

        db.delete(transaction_key(1)).await?;
        let reader = open(&db, objects.clone()).await?;
        let error = for_each_payload(&reader, &records, |_, _, _| Ok(())).await.unwrap_err();
        assert_eq!(error.to_string(), "transaction scan gap/order mismatch");
        reader.close().await?;

        db.delete(key(2)).await?;
        let reader = open(&db, objects.clone()).await?;
        let error = for_each_payload(&reader, &records[1..], |_, _, _| Ok(())).await.unwrap_err();
        assert_eq!(error.to_string(), "block scan gap/order mismatch");
        reader.close().await?;
        db.close().await?;
        Ok(())
    }

    #[tokio::test]
    async fn bounded_metadata_resume_pinning_and_boundary_corruption() -> eyre::Result<()> {
        let store = Store { objects: Arc::new(InMemory::new()), path: Path::from("paged") };
        let mut catalog = catalog(17, 2003, 2);
        let records = std::mem::take(&mut catalog.records);
        catalog.complete = false;
        catalog.checkpoint = None;
        let version = store.publish(&catalog, None).await?;
        let db = Db::open(store.path.clone().join("data"), store.objects.clone()).await?;
        db.put(CONTEXT_KEY, serde_json::to_vec(&catalog.context)?).await?;
        for record in &records[..1000] {
            db.put(metadata_key(record.start), serde_json::to_vec(record)?).await?;
            catalog.cursor.advance(record)?;
        }
        db.put(CURSOR_KEY, serde_json::to_vec(&catalog.cursor)?).await?;
        catalog.checkpoint = Some(store.checkpoint(&db).await?);
        let version = store.publish(&catalog, Some(version)).await?;
        let partial = catalog.clone();
        let (mut resumed, _) = store.catalog().await?.unwrap();
        assert_eq!(resumed.cursor.next, 1017);
        assert!(resumed.records.is_empty());
        for record in &records[1000..] {
            db.put(metadata_key(record.start), serde_json::to_vec(record)?).await?;
            resumed.cursor.advance(record)?;
        }
        db.put(CURSOR_KEY, serde_json::to_vec(&resumed.cursor)?).await?;
        resumed.complete = true;
        resumed.checkpoint = Some(store.checkpoint(&db).await?);
        store.publish(&resumed, Some(version)).await?;
        assert!(serde_json::to_vec(&resumed)?.len() < 2048);
        let old = store.reader(&partial).await?;
        assert!(old.get(metadata_key(1017)).await?.is_none());
        old.close().await?;
        let reader = store.reader(&resumed).await?;
        let mut pages =
            MetadataPages::open(&reader, Cursor::initial(&resumed.context)?, resumed.cursor.next)
                .await?;
        for expected in [1000, 1000, 3] {
            assert_eq!(pages.next_page().await?.len(), expected);
        }
        assert_eq!(pages.cursor, resumed.cursor);
        drop(pages);
        reader.close().await?;
        // A deletion at the page boundary must not be hidden by paging/resume.
        db.delete(metadata_key(1017)).await?;
        resumed.checkpoint = Some(store.checkpoint(&db).await?);
        let corrupt = store.reader(&resumed).await?;
        let mut pages =
            MetadataPages::open(&corrupt, partial.cursor.clone(), resumed.cursor.next).await?;
        assert!(pages.next_page().await.is_err());
        drop(pages);
        corrupt.close().await?;
        // Restoring the key with a wrong parent is also rejected.
        let mut record = records[1000].clone();
        record.parent[0] ^= 1;
        db.put(metadata_key(record.start), serde_json::to_vec(&record)?).await?;
        resumed.checkpoint = Some(store.checkpoint(&db).await?);
        let corrupt = store.reader(&resumed).await?;
        let mut pages = MetadataPages::open(&corrupt, partial.cursor, resumed.cursor.next).await?;
        assert!(pages.next_page().await.is_err());
        drop(pages);
        corrupt.close().await?;
        db.close().await?;
        Ok(())
    }
}
