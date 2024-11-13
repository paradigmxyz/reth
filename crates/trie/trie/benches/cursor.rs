#![allow(missing_docs)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

#[derive(Clone)]
struct ForwardInMemoryCursor {
    entries: Vec<(i32, i32)>,
    index: usize,
}

impl ForwardInMemoryCursor {
    const fn new(entries: Vec<(i32, i32)>) -> Self {
        Self { entries, index: 0 }
    }

    fn linear_advance(&mut self, comparator: impl Fn(&i32) -> bool) -> Option<(i32, i32)> {
        let mut entry = self.entries.get(self.index);
        while entry.map_or(false, |entry| comparator(&entry.0)) {
            self.index += 1;
            entry = self.entries.get(self.index);
        }
        entry.copied()
    }

    fn binary_advance(&mut self, comparator: impl Fn(&i32) -> bool) -> Option<(i32, i32)> {
        if self.index >= self.entries.len() {
            return None;
        }

        let pos = match self.entries[self.index..].binary_search_by(|(k, _)| {
            if comparator(k) {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            }
        }) {
            Ok(found_pos) => self.index + found_pos,
            Err(insert_pos) => self.index + insert_pos,
        };

        let mut first_false = pos;

        while first_false > self.index && !comparator(&self.entries[first_false - 1].0) {
            first_false -= 1;
        }

        self.index = first_false;
        self.entries.get(self.index).copied()
    }

    fn hybrid_advance(&mut self, comparator: impl Fn(&i32) -> bool) -> Option<(i32, i32)> {
        if self.index >= self.entries.len() {
            return None;
        }

        if self.entries.len() - self.index < 100 {
            let mut entry = self.entries.get(self.index);
            while entry.map_or(false, |entry| comparator(&entry.0)) {
                self.index += 1;
                entry = self.entries.get(self.index);
            }
            entry.copied()
        } else {
            let pos = match self.entries[self.index..].binary_search_by(|(k, _)| {
                if comparator(k) {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                }
            }) {
                Ok(found_pos) => self.index + found_pos,
                Err(insert_pos) => self.index + insert_pos,
            };

            let mut first_false = pos;
            while first_false > self.index && !comparator(&self.entries[first_false - 1].0) {
                first_false -= 1;
            }

            self.index = first_false;
            self.entries.get(self.index).copied()
        }
    }
}

fn bench_strategies(c: &mut Criterion) {
    let mut group = c.benchmark_group("advance_strategies");
    for size in [10, 50, 90, 100, 110, 500, 1000] {
        let data = (0..size).map(|i| (i, i)).collect::<Vec<_>>();
        let target = size / 2;
        group.bench_with_input(BenchmarkId::new("linear", size), &data, |b, data| {
            b.iter(|| {
                let mut cursor = ForwardInMemoryCursor::new(data.clone());
                black_box(cursor.linear_advance(|k| *k < target))
            });
        });

        group.bench_with_input(BenchmarkId::new("binary", size), &data, |b, data| {
            b.iter(|| {
                let mut cursor = ForwardInMemoryCursor::new(data.clone());
                black_box(cursor.binary_advance(|k| *k < target))
            });
        });

        group.bench_with_input(BenchmarkId::new("hybrid", size), &data, |b, data| {
            b.iter(|| {
                let mut cursor = ForwardInMemoryCursor::new(data.clone());
                black_box(cursor.hybrid_advance(|k| *k < target))
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_strategies);
criterion_main!(benches);
