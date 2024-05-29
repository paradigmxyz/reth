use super::InclusionFilter;
use crate::NippyJarError;
use cuckoofilter::{CuckooFilter, ExportedCuckooFilter};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::hash_map::DefaultHasher;

/// [CuckooFilter](https://www.cs.cmu.edu/~dga/papers/cuckoo-conext2014.pdf). It builds and provides an approximated set-membership filter to answer queries such as "Does this element belong to this set?". Has a theoretical 3% false positive rate.
pub struct Cuckoo {
    /// Remaining number of elements that can be added.
    ///
    /// This is necessary because the inner implementation will fail on adding an element past capacity, **but it will still add it and remove other**: [source](https://github.com/axiomhq/rust-cuckoofilter/tree/624da891bed1dd5d002c8fa92ce0dcd301975561#notes--todos)
    remaining: usize,

    /// CuckooFilter.
    filter: CuckooFilter<DefaultHasher>, // TODO does it need an actual hasher?
}

impl Cuckoo {
    pub fn new(max_capacity: usize) -> Self {
        // CuckooFilter might return `NotEnoughSpace` even if they are remaining elements, if it's
        // close to capacity. Therefore, we increase it.
        let max_capacity = max_capacity + 100 + max_capacity / 3;

        Cuckoo { remaining: max_capacity, filter: CuckooFilter::with_capacity(max_capacity) }
    }
}

impl InclusionFilter for Cuckoo {
    fn add(&mut self, element: &[u8]) -> Result<(), NippyJarError> {
        if self.remaining == 0 {
            return Err(NippyJarError::FilterMaxCapacity)
        }

        self.remaining -= 1;

        Ok(self.filter.add(element)?)
    }

    fn contains(&self, element: &[u8]) -> Result<bool, NippyJarError> {
        Ok(self.filter.contains(element))
    }

    fn size(&self) -> usize {
        self.filter.memory_usage()
    }
}

impl std::fmt::Debug for Cuckoo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cuckoo")
            .field("remaining", &self.remaining)
            .field("filter_size", &self.filter.memory_usage())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
impl PartialEq for Cuckoo {
    fn eq(&self, _other: &Self) -> bool {
        self.remaining == _other.remaining && {
            let f1 = self.filter.export();
            let f2 = _other.filter.export();
            f1.length == f2.length && f1.values == f2.values
        }
    }
}

impl<'de> Deserialize<'de> for Cuckoo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let (remaining, exported): (usize, ExportedCuckooFilter) =
            Deserialize::deserialize(deserializer)?;

        Ok(Cuckoo { remaining, filter: exported.into() })
    }
}

impl Serialize for Cuckoo {
    /// Potentially expensive, but should be used only when creating the file.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        (self.remaining, self.filter.export()).serialize(serializer)
    }
}
