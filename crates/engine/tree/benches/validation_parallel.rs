//! Benchmark for parallel block validation operations.

#![allow(missing_docs)]

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::time::Instant;

/// Mock validation task that simulates CPU-bound work similar to block validation
fn mock_block_validation() -> Result<(), &'static str> {
    let mut sum = 0u64;
    
    // Simulate cryptographic validation work
    for i in 0..50_000 {
        sum = sum.wrapping_add(i.wrapping_mul(7).wrapping_add(3));
        // Add some branching to prevent optimization
        if i % 1000 == 0 {
            sum = sum.wrapping_mul(13);
        }
    }
    
    // Simulate some validation logic
    if sum % 2 == 0 { 
        Ok(()) 
    } else { 
        Err("Validation failed") 
    }
}

/// Mock parent validation task that simulates CPU-bound work similar to parent header validation
fn mock_parent_validation() -> Result<(), &'static str> {
    let mut product = 1u64;
    
    // Simulate header validation work against parent
    for i in 1..30_000 {
        product = product.wrapping_mul(i % 17 + 1);
        // Add some branching to prevent optimization  
        if i % 500 == 0 {
            product = product.wrapping_add(i * 3);
        }
    }
    
    // Simulate some validation logic
    if product % 3 == 0 { 
        Ok(()) 
    } else { 
        Err("Parent validation failed") 
    }
}

/// Benchmark sequential validation (original approach)
fn bench_sequential_validation(c: &mut Criterion) {
    c.bench_function("sequential_validation", |b| {
        b.iter(|| {
            let result1 = black_box(mock_block_validation());
            let result2 = black_box(mock_parent_validation());
            (result1, result2)
        })
    });
}

/// Benchmark parallel validation (new approach with rayon::join)
fn bench_parallel_validation(c: &mut Criterion) {
    c.bench_function("parallel_validation", |b| {
        b.iter(|| {
            let (result1, result2) = rayon::join(
                || black_box(mock_block_validation()),
                || black_box(mock_parent_validation())
            );
            (result1, result2)
        })
    });
}

criterion_group!(benches, bench_sequential_validation, bench_parallel_validation);
criterion_main!(benches);