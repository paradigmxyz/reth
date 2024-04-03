#![allow(missing_docs)]
use criterion::{black_box, criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use rand::prelude::*;

pub fn new_pre_sorted(c: &mut Criterion) {
    let mut group = c.benchmark_group("new_pre_sorted");

    for delta in [1, 100, 1000, 10000] {
        let integers_usize = generate_integers(2000, delta);
        assert_eq!(integers_usize.len(), 2000);

        let integers_u64 = integers_usize.iter().map(|v| *v as u64).collect::<Vec<_>>();
        assert_eq!(integers_u64.len(), 2000);

        group.bench_function(BenchmarkId::new("Elias-Fano", delta), |b| {
            b.iter(|| elias_fano::IntegerList::new_pre_sorted(black_box(&integers_usize)));
        });

        group.bench_function(BenchmarkId::new("Roaring Bitmaps", delta), |b| {
            b.iter(|| reth_primitives::IntegerList::new_pre_sorted(black_box(&integers_u64)));
        });
    }
}

pub fn rank_select(c: &mut Criterion) {
    let mut group = c.benchmark_group("rank + select");

    for delta in [1, 100, 1000, 10000] {
        let integers_usize = generate_integers(2000, delta);
        assert_eq!(integers_usize.len(), 2000);

        let integers_u64 = integers_usize.iter().map(|v| *v as u64).collect::<Vec<_>>();
        assert_eq!(integers_u64.len(), 2000);

        group.bench_function(BenchmarkId::new("Elias-Fano", delta), |b| {
            b.iter_batched(
                || {
                    let (index, element) =
                        integers_usize.iter().enumerate().choose(&mut thread_rng()).unwrap();
                    (elias_fano::IntegerList::new_pre_sorted(&integers_usize).0, index, *element)
                },
                |(list, index, element)| {
                    let list = list.enable_rank();
                    list.rank(element);
                    list.select(index);
                },
                BatchSize::PerIteration,
            );
        });

        group.bench_function(BenchmarkId::new("Roaring Bitmaps", delta), |b| {
            b.iter_batched(
                || {
                    let (index, element) =
                        integers_u64.iter().enumerate().choose(&mut thread_rng()).unwrap();
                    (
                        reth_primitives::IntegerList::new_pre_sorted(&integers_u64),
                        index as u64,
                        *element,
                    )
                },
                |(list, index, element)| {
                    list.rank(element);
                    list.select(index);
                },
                BatchSize::PerIteration,
            );
        });
    }
}

fn generate_integers(n: usize, delta: usize) -> Vec<usize> {
    (0..n).fold(Vec::new(), |mut vec, _| {
        vec.push(vec.last().map_or(0, |last| {
            last + thread_rng().gen_range(delta - delta / 2..=delta + delta / 2)
        }));
        vec
    })
}

criterion_group! {
    name = benches;
    config = Criterion::default();
    targets = new_pre_sorted, rank_select
}
criterion_main!(benches);

/// Implementation from https://github.com/paradigmxyz/reth/blob/cda5d4e7c53ccc898b7725eb5d3b46c35e4da7f8/crates/primitives/src/integer_list.rs
/// adapted to work with `sucds = "0.8.1"`
#[allow(unused, unreachable_pub)]
mod elias_fano {
    use std::{fmt, ops::Deref};
    use sucds::{mii_sequences::EliasFano, Serializable};

    #[derive(Clone, PartialEq, Eq, Default)]
    pub struct IntegerList(pub EliasFano);

    impl Deref for IntegerList {
        type Target = EliasFano;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl fmt::Debug for IntegerList {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            let vec: Vec<usize> = self.0.iter(0).collect();
            write!(f, "IntegerList {vec:?}")
        }
    }

    impl IntegerList {
        /// Creates an IntegerList from a list of integers. `usize` is safe to use since
        /// [`sucds::EliasFano`] restricts its compilation to 64bits.
        ///
        /// # Returns
        ///
        /// Returns an error if the list is empty or not pre-sorted.
        pub fn new<T: AsRef<[usize]>>(list: T) -> Result<Self, EliasFanoError> {
            let mut builder = EliasFanoBuilder::new(
                list.as_ref().iter().max().map_or(0, |max| max + 1),
                list.as_ref().len(),
            )?;
            builder.extend(list.as_ref().iter().copied());
            Ok(Self(builder.build()))
        }

        // Creates an IntegerList from a pre-sorted list of integers. `usize` is safe to use since
        /// [`sucds::EliasFano`] restricts its compilation to 64bits.
        ///
        /// # Panics
        ///
        /// Panics if the list is empty or not pre-sorted.
        pub fn new_pre_sorted<T: AsRef<[usize]>>(list: T) -> Self {
            Self::new(list).expect("IntegerList must be pre-sorted and non-empty.")
        }

        /// Serializes a [`IntegerList`] into a sequence of bytes.
        pub fn to_bytes(&self) -> Vec<u8> {
            let mut vec = Vec::with_capacity(self.0.size_in_bytes());
            self.0.serialize_into(&mut vec).expect("not able to encode integer list.");
            vec
        }

        /// Serializes a [`IntegerList`] into a sequence of bytes.
        pub fn to_mut_bytes<B: bytes::BufMut>(&self, buf: &mut B) {
            let len = self.0.size_in_bytes();
            let mut vec = Vec::with_capacity(len);
            self.0.serialize_into(&mut vec).unwrap();
            buf.put_slice(vec.as_slice());
        }

        /// Deserializes a sequence of bytes into a proper [`IntegerList`].
        pub fn from_bytes(data: &[u8]) -> Result<Self, EliasFanoError> {
            Ok(Self(
                EliasFano::deserialize_from(data).map_err(|_| EliasFanoError::FailedDeserialize)?,
            ))
        }
    }

    macro_rules! impl_uint {
        ($($w:tt),+) => {
            $(
                impl From<Vec<$w>> for IntegerList {
                    fn from(v: Vec<$w>) -> Self {
                        let v: Vec<usize> = v.iter().map(|v| *v as usize).collect();
                        Self::new(v.as_slice()).expect("could not create list.")
                    }
                }
            )+
        };
    }

    impl_uint!(usize, u64, u32, u8, u16);

    impl Serialize for IntegerList {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let vec = self.0.iter(0).collect::<Vec<usize>>();
            let mut seq = serializer.serialize_seq(Some(self.len()))?;
            for e in vec {
                seq.serialize_element(&e)?;
            }
            seq.end()
        }
    }

    struct IntegerListVisitor;
    impl<'de> Visitor<'de> for IntegerListVisitor {
        type Value = IntegerList;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a usize array")
        }

        fn visit_seq<E>(self, mut seq: E) -> Result<Self::Value, E::Error>
        where
            E: SeqAccess<'de>,
        {
            let mut list = Vec::new();
            while let Some(item) = seq.next_element()? {
                list.push(item);
            }

            IntegerList::new(list)
                .map_err(|_| serde::de::Error::invalid_value(Unexpected::Seq, &self))
        }
    }

    impl<'de> Deserialize<'de> for IntegerList {
        fn deserialize<D>(deserializer: D) -> Result<IntegerList, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_byte_buf(IntegerListVisitor)
        }
    }

    #[cfg(any(test, feature = "arbitrary"))]
    use arbitrary::{Arbitrary, Unstructured};
    use serde::{
        de::{SeqAccess, Unexpected, Visitor},
        ser::SerializeSeq,
        Deserialize, Deserializer, Serialize, Serializer,
    };
    use sucds::mii_sequences::EliasFanoBuilder;

    #[cfg(any(test, feature = "arbitrary"))]
    impl<'a> Arbitrary<'a> for IntegerList {
        fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self, arbitrary::Error> {
            let mut nums: Vec<usize> = Vec::arbitrary(u)?;
            nums.sort();
            Self::new(&nums).map_err(|_| arbitrary::Error::IncorrectFormat)
        }
    }

    /// Primitives error type.
    #[derive(Debug, thiserror::Error)]
    pub enum EliasFanoError {
        /// The provided input is invalid.
        #[error(transparent)]
        InvalidInput(#[from] anyhow::Error),
        /// Failed to deserialize data into type.
        #[error("failed to deserialize data into type")]
        FailedDeserialize,
    }
}
