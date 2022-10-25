use crate::{ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput};
use reth_interfaces::{
    consensus::{Consensus, ForkchoiceState},
    db::{
        models::blocks::BlockNumHash, tables, DBContainer, Database, DatabaseGAT, DbCursorRO,
        DbCursorRW, DbTx, DbTxMut,
    },
    p2p::headers::{
        client::HeadersClient,
        downloader::{DownloadError, Downloader},
    },
};
use reth_primitives::{rpc::BigEndianHash, BlockNumber, HeaderLocked, H256, U256};
use std::fmt::Debug;
use thiserror::Error;
use tracing::*;

const HEADERS: StageId = StageId("HEADERS");

/// The headers stage implementation for staged sync
#[derive(Debug)]
pub struct HeaderStage<D: Downloader, C: Consensus, H: HeadersClient> {
    /// Strategy for downloading the headers
    pub downloader: D,
    /// Consensus client implementation
    pub consensus: C,
    /// Downloader client implementation
    pub client: H,
}

/// The header stage error
#[derive(Error, Debug)]
pub enum HeaderStageError {
    /// Cannonical hash is missing from db
    #[error("no cannonical hash for block #{number}")]
    NoCannonicalHash {
        /// The block number key
        number: BlockNumber,
    },
    /// Cannonical header is missing from db
    #[error("no cannonical hash for block #{number}")]
    NoCannonicalHeader {
        /// The block number key
        number: BlockNumber,
    },
    /// Header is missing from db
    #[error("no header for block #{number} ({hash})")]
    NoHeader {
        /// The block number key
        number: BlockNumber,
        /// The block hash key
        hash: H256,
    },
}

#[async_trait::async_trait]
impl<DB: Database, D: Downloader, C: Consensus, H: HeadersClient> Stage<DB>
    for HeaderStage<D, C, H>
{
    /// Return the id of the stage
    fn id(&self) -> StageId {
        HEADERS
    }

    /// Download the headers in reverse order
    /// starting from the tip
    async fn execute(
        &mut self,
        db: &mut DBContainer<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let tx = db.get_mut();
        let last_block_num =
            input.previous_stage.as_ref().map(|(_, block)| *block).unwrap_or_default();
        // TODO: check if in case of panic the node head needs to be updated
        self.update_head::<DB>(tx, last_block_num).await?;

        // download the headers
        // TODO: check if some upper block constraint is necessary
        let last_hash =
            tx.get::<tables::CanonicalHeaders>(last_block_num)?.ok_or_else(|| -> StageError {
                HeaderStageError::NoCannonicalHash { number: last_block_num }.into()
            })?;
        let last_header = tx
            .get::<tables::Headers>((last_block_num, last_hash).into())?
            .ok_or_else(|| -> StageError {
                HeaderStageError::NoHeader { number: last_block_num, hash: last_hash }.into()
            })?;
        let head = HeaderLocked::new(last_header, last_hash);

        let forkchoice = self.next_fork_choice_state(&head.hash()).await;
        // The stage relies on the downloader to return the headers
        // in descending order starting from the tip down to
        // the local head (latest block in db)
        let headers = match self.downloader.download(&head, &forkchoice).await {
            Ok(res) => {
                // TODO: validate the result order?
                // at least check if it attaches (first == tip && last == last_hash)
                res
            }
            Err(e) => match e {
                DownloadError::Timeout { request_id } => {
                    warn!("no response for header request {request_id}");
                    return Ok(ExecOutput {
                        stage_progress: last_block_num,
                        reached_tip: false,
                        done: false,
                    })
                }
                DownloadError::HeaderValidation { hash, details } => {
                    warn!("validation error for header {hash}: {details}");
                    return Err(StageError::Validation { block: last_block_num })
                }
                // TODO: this error is never propagated, clean up
                DownloadError::MismatchedHeaders { .. } => todo!(),
            },
        };

        let stage_progress = self.write_headers::<DB>(tx, headers).await?;
        Ok(ExecOutput { stage_progress, reached_tip: true, done: true })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        db: &mut DBContainer<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>> {
        let tx = db.get_mut();
        if let Some(_bad_block) = input.bad_block {
            todo!()
        }

        // TODO: blocked by https://github.com/foundry-rs/reth/pull/122
        // let mut walker = tx.cursor_mut::<tables::CanonicalHeaders>()?.walk(input.unwind_to + 1)?;
        // while let Some(key) = walker.next().transpose()? {
        //     tx.delete::<tables::HeaderNumbers>(key.into(), None)?;
        // }

        // TODO: cleanup
        let mut cur = tx.cursor_mut::<tables::Headers>()?;
        let mut entry = cur.last()?;
        while let Some((BlockNumHash((num, _)), _)) = entry {
            if num <= input.unwind_to {
                break
            }
            cur.delete_current()?;
            entry = cur.prev()?;
        }

        let mut cur = tx.cursor_mut::<tables::CanonicalHeaders>()?;
        let mut entry = cur.last()?;
        while let Some((block_num, _)) = entry {
            if block_num <= input.unwind_to {
                break
            }
            cur.delete_current()?;
            entry = cur.prev()?;
        }

        let mut cur = tx.cursor_mut::<tables::HeaderTD>()?;
        let mut entry = cur.last()?;
        while let Some((BlockNumHash((num, _)), _)) = entry {
            if num <= input.unwind_to {
                break
            }
            cur.delete_current()?;
            entry = cur.prev()?;
        }

        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}

impl<D: Downloader, C: Consensus, H: HeadersClient> HeaderStage<D, C, H> {
    async fn update_head<DB: Database>(
        &self,
        tx: &mut <DB as DatabaseGAT<'_>>::TXMut,
        height: BlockNumber,
    ) -> Result<(), StageError> {
        let hash = tx.get::<tables::CanonicalHeaders>(height)?.ok_or_else(|| -> StageError {
            HeaderStageError::NoCannonicalHeader { number: height }.into()
        })?;
        let td: Vec<u8> = tx.get::<tables::HeaderTD>((height, hash).into())?.unwrap(); // TODO:
        self.client.update_status(height, hash, H256::from_slice(&td)).await;
        Ok(())
    }

    async fn next_fork_choice_state(&self, head: &H256) -> ForkchoiceState {
        let mut state_rcv = self.consensus.fork_choice_state();
        loop {
            let _ = state_rcv.changed().await;
            let forkchoice = state_rcv.borrow();
            if !forkchoice.head_block_hash.is_zero() && forkchoice.head_block_hash != *head {
                return forkchoice.clone()
            }
        }
    }

    /// Write downloaded headers to the database
    async fn write_headers<DB: Database>(
        &self,
        tx: &mut <DB as DatabaseGAT<'_>>::TXMut,
        headers: Vec<HeaderLocked>,
    ) -> Result<BlockNumber, StageError> {
        let mut cursor_header_number = tx.cursor_mut::<tables::HeaderNumbers>()?;
        let mut cursor_header = tx.cursor_mut::<tables::Headers>()?;
        let mut cursor_canonical = tx.cursor_mut::<tables::CanonicalHeaders>()?;
        let mut cursor_td = tx.cursor_mut::<tables::HeaderTD>()?;
        let mut td = U256::from_big_endian(&cursor_td.last()?.map(|(_, v)| v).unwrap());

        // TODO: comment
        let mut latest = 0;
        // Since the headers were returned in descending order,
        // iterate them in the reverse order
        for header in headers.into_iter().rev() {
            if header.number == 0 {
                continue
            }

            let key: BlockNumHash = (header.number, header.hash()).into();
            let header = header.unlock();
            latest = header.number;

            td += header.difficulty;

            // TODO: investigate default write flags
            cursor_header_number.append(key, header.number)?;
            cursor_header.append(key, header)?;
            cursor_canonical.append(key.0 .0, key.0 .1)?;
            cursor_td.append(
                key,
                H256::from_uint(&td).as_bytes().to_vec(), // TODO:
            )?;
        }

        Ok(latest)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use once_cell::sync::Lazy;
    use reth_db::{kv::Env, mdbx::WriteMap};
    use reth_interfaces::{
        db::DBContainer,
        test_utils::{
            gen_random_header, gen_random_header_range, TestConsensus, TestHeadersClient,
        },
    };
    use std::{borrow::Borrow, sync::Arc};
    use test_utils::HeadersDB;
    use tokio::sync::oneshot;

    static CONSENSUS: Lazy<TestConsensus> = Lazy::new(|| TestConsensus::default());
    static CLIENT: Lazy<TestHeadersClient> = Lazy::new(|| TestHeadersClient::default());

    #[tokio::test]
    async fn headers_execute_empty_db() {
        let db = HeadersDB::default();
        let input = ExecInput { previous_stage: None, stage_progress: None };
        let rx = run_stage(db.inner(), input, Ok(vec![]));
        assert_matches!(
            rx.await.unwrap(),
            Err(StageError::HeadersStage(HeaderStageError::NoCannonicalHeader { .. }))
        );
    }

    #[tokio::test]
    async fn headers_execute_no_response() {
        let head = gen_random_header(0, None);
        let db = HeadersDB::default();
        db.insert_header(&head).expect("failed to insert header");

        let input = ExecInput { previous_stage: None, stage_progress: None };
        let rx = run_stage(db.inner(), input, Err(DownloadError::Timeout { request_id: 0 }));
        CONSENSUS.update_tip(H256::from_low_u64_be(1));
        assert_matches!(rx.await.unwrap(), Ok(ExecOutput { done, .. }) if !done);
    }

    #[tokio::test]
    async fn headers_execute_validation_error() {
        let head = gen_random_header(0, None);
        let db = HeadersDB::default();
        db.insert_header(&head).expect("failed to insert header");

        let input = ExecInput { previous_stage: None, stage_progress: None };
        let rx = run_stage(
            db.inner(),
            input,
            Err(DownloadError::HeaderValidation { hash: H256::zero(), details: "".to_owned() }),
        );
        CONSENSUS.update_tip(H256::from_low_u64_be(1));

        assert_matches!(rx.await.unwrap(), Err(StageError::Validation { block }) if block == 0);
    }

    #[tokio::test]
    async fn headers_execute_no_progress() {
        let head = gen_random_header(0, None);
        let headers = gen_random_header_range(1..100, head.hash());
        let db = HeadersDB::default();
        db.insert_header(&head).expect("failed to insert header");

        let result: Vec<_> = headers.iter().rev().cloned().collect();
        let input = ExecInput { previous_stage: None, stage_progress: None };
        let rx = run_stage(db.inner(), input, Ok(result));
        let tip = headers.last().unwrap();
        CONSENSUS.update_tip(tip.hash());

        let result = rx.await.unwrap();
        assert_matches!(result, Ok(ExecOutput { .. }));
        let result = result.unwrap();
        assert!(result.done && result.reached_tip);
        assert_eq!(result.stage_progress, tip.number);

        for header in headers {
            assert!(db.validate_db_header(&header).is_ok());
        }
    }

    fn run_stage(
        db: Arc<Env<WriteMap>>,
        input: ExecInput,
        download_result: Result<Vec<HeaderLocked>, DownloadError>,
    ) -> oneshot::Receiver<Result<ExecOutput, StageError>> {
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let db = db.clone();
            let mut db = DBContainer::<Env<WriteMap>>::new(db.borrow()).unwrap();
            let mut stage = HeaderStage {
                client: &*CLIENT,
                consensus: &*CONSENSUS,
                downloader: test_utils::TestDownloader::new(download_result),
            };
            let result = stage.execute(&mut db, input).await;
            db.commit().expect("failed to commit");
            tx.send(result).expect("failed to send result");
        });
        rx
    }

    pub(crate) mod test_utils {
        use async_trait::async_trait;
        use reth_db::{
            kv::{test_utils::create_test_db, Env, EnvKind},
            mdbx,
            mdbx::WriteMap,
        };
        use reth_interfaces::{
            consensus::ForkchoiceState,
            db::{
                self, models::blocks::BlockNumHash, tables, DBContainer, DbCursorRO, DbCursorRW,
                DbTx, DbTxMut,
            },
            p2p::headers::downloader::{DownloadError, Downloader},
            test_utils::{TestConsensus, TestHeadersClient},
        };
        use reth_primitives::{rpc::BigEndianHash, HeaderLocked, H256, U256};
        use std::{borrow::Borrow, sync::Arc, time::Duration};

        pub(crate) struct HeadersDB {
            db: Arc<Env<WriteMap>>,
        }

        impl Default for HeadersDB {
            fn default() -> Self {
                Self { db: Arc::new(create_test_db::<mdbx::WriteMap>(EnvKind::RW)) }
            }
        }

        impl HeadersDB {
            pub(crate) fn inner(&self) -> Arc<Env<WriteMap>> {
                self.db.clone()
            }

            fn container(&self) -> DBContainer<'_, Env<WriteMap>> {
                DBContainer::new(self.db.borrow()).expect("failed to create db container")
            }

            pub(crate) fn insert_header(&self, header: &HeaderLocked) -> Result<(), db::Error> {
                let mut db = self.container();
                let tx = db.get_mut();

                let key: BlockNumHash = (header.number, header.hash()).into();
                tx.put::<tables::HeaderNumbers>(key, header.number)?;
                tx.put::<tables::Headers>(key, header.clone().unlock())?;
                tx.put::<tables::CanonicalHeaders>(header.number, header.hash())?;

                let mut cursor_td = tx.cursor_mut::<tables::HeaderTD>()?;
                let td =
                    U256::from_big_endian(&cursor_td.last()?.map(|(_, v)| v).unwrap_or(vec![]));
                cursor_td
                    .append(key, H256::from_uint(&(td + header.difficulty)).as_bytes().to_vec())?;

                db.commit()?;
                Ok(())
            }

            pub(crate) fn validate_db_header(
                &self,
                header: &HeaderLocked,
            ) -> Result<(), db::Error> {
                let db = self.container();
                let tx = db.get();
                let key: BlockNumHash = (header.number, header.hash()).into();

                let db_number = tx.get::<tables::HeaderNumbers>(key)?;
                assert_eq!(db_number, Some(header.number));

                let db_header = tx.get::<tables::Headers>(key)?;
                assert_eq!(db_header, Some(header.clone().unlock()));

                let db_canonical_header = tx.get::<tables::CanonicalHeaders>(header.number)?;
                assert_eq!(db_canonical_header, Some(header.hash()));

                if header.number != 0 {
                    let parent_key: BlockNumHash = (header.number - 1, header.parent_hash).into();
                    let parent_td = tx.get::<tables::HeaderTD>(parent_key)?;
                    let td = U256::from_big_endian(&tx.get::<tables::HeaderTD>(key)?.unwrap());
                    assert_eq!(
                        parent_td.map(|td| U256::from_big_endian(&td) + header.difficulty),
                        Some(td)
                    );
                }

                Ok(())
            }
        }

        #[derive(Debug)]
        pub(crate) struct TestDownloader {
            result: Result<Vec<HeaderLocked>, DownloadError>,
        }

        impl TestDownloader {
            pub(crate) fn new(result: Result<Vec<HeaderLocked>, DownloadError>) -> Self {
                Self { result }
            }
        }

        #[async_trait]
        impl Downloader for TestDownloader {
            type Consensus = TestConsensus;
            type Client = TestHeadersClient;

            fn timeout(&self) -> Duration {
                Duration::from_secs(1)
            }

            fn consensus(&self) -> &Self::Consensus {
                unimplemented!()
            }

            fn client(&self) -> &Self::Client {
                unimplemented!()
            }

            async fn download(
                &self,
                _: &HeaderLocked,
                _: &ForkchoiceState,
            ) -> Result<Vec<HeaderLocked>, DownloadError> {
                self.result.clone()
            }
        }
    }
}
