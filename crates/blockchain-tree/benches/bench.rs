#![allow(missing_docs)]
#![allow(clippy::field_reassign_with_default)]
use alloy_consensus::Header;
use alloy_primitives::BlockHash;
use criterion::{self, black_box, criterion_group, criterion_main, Criterion};
use reth_blockchain_tree::BlockBuffer;
use reth_primitives::{BlockBody, SealedBlock, SealedBlockWithSenders, SealedHeader};

pub fn create_block(_number: u64, _parent: BlockHash) -> SealedBlockWithSenders {
    let mut header = Header::default();
    header.number = _number;
    header.parent_hash = _parent;

    let block_hash = BlockHash::random();
    let s_header = SealedHeader::new(header, block_hash);

    let body = BlockBody::default();
    let sealed_block = SealedBlock::new(s_header, body);
    SealedBlockWithSenders::new(sealed_block, vec![]).unwrap()
}

pub fn setup_buffer(size: usize) -> BlockBuffer {
    let mut buffer = BlockBuffer::new(size as u32);
    let _rng = rand::thread_rng();
    let mut parent = BlockHash::random();

    for i in 0..size {
        let number = i as u64;
        let block = create_block(number, parent);
        parent = block.hash();
        buffer.insert_block(block);
    }

    buffer
}

fn bench_remove_old_blocks(c: &mut Criterion) {
    let mut group = c.benchmark_group("remove_old_blocks");

    for size in &[100, 1000, 10000] {
        let mut i = 25;
        group.bench_function(format!("size_{}", size), |b| {
            b.iter_with_setup(
                || setup_buffer(*size),
                |mut buffer| {
                    buffer.remove_old_blocks(black_box((size - i) as u64));
                },
            )
        });
        i *= 10;
    }

    group.finish();
}

criterion_group!(benches, bench_remove_old_blocks);
criterion_main!(benches);
