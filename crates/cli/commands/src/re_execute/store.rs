//! Pinned corpus publication and read-only checkpoint access.

use async_trait::async_trait;
use aws_credential_types::provider::{ProvideCredentials, SharedCredentialsProvider};
use bytes::Bytes;
use futures::TryStreamExt;
use object_store::{
    aws::{AmazonS3Builder, AwsCredential},
    path::Path,
    CredentialProvider, ObjectStore, ObjectStoreExt, PutMode, PutOptions, UpdateVersion,
};
use serde::{Deserialize, Serialize};
use slatedb::{
    config::{CheckpointOptions, CheckpointScope, DbReaderOptions, FlushOptions, FlushType},
    Db, DbReader, DbReaderMode,
};
use std::sync::Arc;
use url::Url;

/// Serialized records, including in-flight reads and records executing on workers.
/// Does not cap decoded EVM state, SlateDB scan/cache buffers, or SDK allocations.
pub(super) const BYTE_BUDGET: usize = 64 * 1024 * 1024;
const MAX_CATALOG_BYTES: usize = 16 * 1024 * 1024;
pub(super) const CONTEXT_KEY: &[u8] = b"reth-corpus-context";

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
    pub chunk_blocks: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Record {
    pub start: u64,
    pub end: u64,
    pub parent: [u8; 32],
    pub tip: [u8; 32],
    pub bytes: usize,
    pub digest: String,
    pub transactions: u64,
    pub gas: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Catalog {
    pub context: Context,
    pub checkpoint: Option<String>,
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
            c.schema == 1 && c.start > 0 && c.end > c.start && c.chunk_blocks > 0,
            "unsupported corpus context/range"
        );
        let mut next = c.start;
        let mut parent = c.parent.parse::<alloy_primitives::B256>()?.0;
        for record in &self.records {
            eyre::ensure!(
                record.start == next &&
                    record.start < c.end &&
                    record.end == record.start.saturating_add(c.chunk_blocks).min(c.end),
                "catalog gap/duplicate/chunk mismatch"
            );
            eyre::ensure!(
                record.parent == parent && record.bytes > 0 && record.bytes <= BYTE_BUDGET,
                "catalog hash chain or record size mismatch"
            );
            next = record.end;
            parent = record.tip;
        }
        eyre::ensure!(
            !self.complete ||
                (next == c.end && parent == c.tip.parse::<alloy_primitives::B256>()?.0),
            "incomplete corpus publication"
        );
        eyre::ensure!(
            self.records.is_empty() || self.checkpoint.is_some(),
            "missing pinned checkpoint"
        );
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
            "s3" => {
                let bucket = url.host_str().ok_or_else(|| eyre::eyre!("missing S3 bucket"))?;
                let prefix = url.path().trim_matches('/');
                eyre::ensure!(!prefix.is_empty(), "a non-root corpus prefix is required");
                let sdk = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
                let credentials = sdk
                    .credentials_provider()
                    .ok_or_else(|| eyre::eyre!("AWS credential provider unavailable"))?;
                let mut builder = AmazonS3Builder::from_env()
                    .with_bucket_name(bucket)
                    .with_region(sdk.region().map(|r| r.as_ref()).unwrap_or("us-east-1"))
                    .with_credentials(Arc::new(Credentials(credentials)));
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
                    path: Path::from(prefix).join("corpus-v1"),
                })
            }
            _ => eyre::bail!("corpus must use s3://bucket/prefix"),
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
            DbReaderOptions { object_store_max_retries: Some(3), ..Default::default() },
        )
        .await?;
        let checked = async {
            let bytes = reader
                .get(CONTEXT_KEY)
                .await?
                .ok_or_else(|| eyre::eyre!("checkpoint has no corpus context"))?;
            let context: Context = serde_json::from_slice(&bytes)?;
            eyre::ensure!(context == catalog.context, "checkpoint/catalog context mismatch");
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

pub(super) fn key(start: u64) -> [u8; 8] {
    start.to_be_bytes()
}

pub(super) fn decode(bytes: &[u8], record: &Record) -> eyre::Result<Arc<super::chunk::Chunk>> {
    eyre::ensure!(
        bytes.len() == record.bytes && blake3::hash(bytes).to_hex().as_str() == record.digest,
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use object_store::memory::InMemory;
    use proptest::prelude::*;

    fn catalog(start: u64, count: usize, chunk_blocks: u64) -> Catalog {
        let records: Vec<_> = (0..count)
            .map(|i| Record {
                start: start + i as u64 * chunk_blocks,
                end: start + (i as u64 + 1) * chunk_blocks,
                parent: [i as u8; 32],
                tip: [i as u8 + 1; 32],
                bytes: 1,
                digest: blake3::hash(b"a").to_hex().to_string(),
                transactions: 0,
                gas: 0,
            })
            .collect();
        Catalog {
            context: Context {
                schema: 1,
                chain_id: 1,
                genesis: B256::ZERO.to_string(),
                forks: "test".to_owned(),
                start,
                end: start + count as u64 * chunk_blocks,
                parent: B256::ZERO.to_string(),
                tip: B256::from([count as u8; 32]).to_string(),
                chunk_blocks,
            },
            records,
            checkpoint: Some("00000000-0000-0000-0000-000000000000".to_owned()),
            complete: true,
            generation: 1,
        }
    }

    proptest! {
        #[test]
        fn coverage_rejects_missing_extra_and_reordered_chunks(start in 1u64..1_000_000, count in 2usize..64, size in 1u64..100) {
            let expected = catalog(start, count, size);
            prop_assert!(expected.validate().is_ok());
            let encoded = serde_json::to_vec(&expected).unwrap();
            let roundtrip: Catalog = serde_json::from_slice(&encoded).unwrap();
            prop_assert!(roundtrip.validate().is_ok());
            let mut missing = expected.clone();
            missing.records.remove(count/2);
            prop_assert!(missing.validate().is_err());
            let mut extra = expected.clone();
            extra.records.push(extra.records.last().unwrap().clone());
            prop_assert!(extra.validate().is_err());
            let mut reordered = expected;
            reordered.records.swap(0, 1);
            prop_assert!(reordered.validate().is_err());
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
}
