#![allow(unused)]
//! Test helper impls for generating bodies
use reth_db::{
    database::Database,
    mdbx::{Env, WriteMap},
    tables,
    transaction::DbTxMut,
};
use reth_eth_wire::{BlockBody, RawBlockBody};
use reth_interfaces::{db, p2p::bodies::response::BlockResponse};
use reth_primitives::{SealedBlock, SealedHeader, H256};
use std::collections::HashMap;

pub(crate) fn zip_blocks<'a>(
    headers: impl Iterator<Item = &'a SealedHeader>,
    bodies: &mut HashMap<H256, BlockBody>,
) -> Vec<BlockResponse> {
    headers
        .into_iter()
        .map(|header| {
            let body = bodies.remove(&header.hash()).expect("body exists");
            if header.is_empty() {
                BlockResponse::Empty(header.clone())
            } else {
                BlockResponse::Full(SealedBlock {
                    header: header.clone(),
                    body: body.transactions,
                    ommers: body.ommers.into_iter().map(|o| o.seal()).collect(),
                })
            }
        })
        .collect()
}

pub(crate) fn create_raw_bodies<'a>(
    headers: impl Iterator<Item = &'a SealedHeader>,
    bodies: &mut HashMap<H256, BlockBody>,
) -> Vec<RawBlockBody> {
    headers
        .into_iter()
        .map(|header| {
            let body = bodies.remove(&header.hash()).expect("body exists");
            body.create_block(header)
        })
        .collect()
}

#[inline]
pub(crate) fn insert_headers(db: &Env<WriteMap>, headers: &[SealedHeader]) {
    db.update(|tx| -> Result<(), db::Error> {
        for header in headers {
            tx.put::<tables::CanonicalHeaders>(header.number, header.hash())?;
            tx.put::<tables::Headers>(header.num_hash().into(), header.clone().unseal())?;
        }
        Ok(())
    })
    .expect("failed to commit")
    .expect("failed to insert headers");
}
