use super::Filter;
use crate::NippyJarError;
use cuckoofilter::{self, CuckooFilter, ExportedCuckooFilter};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::hash_map::DefaultHasher;

/// [CuckooFilter](https://www.cs.cmu.edu/~dga/papers/cuckoo-conext2014.pdf). It builds and provides an approximated set-membership filter to answer queries such as "Does this element belong to this set?". Has a theoretical 3% false positive rate.
pub struct Cuckoo {
    /// Remaining number of elements that can be added. This is necessary because the inner implementation will fail on adding an element past capacity, **but it will still add it and remove other**: [source](https://github.com/axiomhq/rust-cuckoofilter/tree/624da891bed1dd5d002c8fa92ce0dcd301975561#notes--todos)
    remaining: usize,

    /// CuckooFilter.
    filter: Option<CuckooFilter<DefaultHasher>>, // TODO does it need an actual hasher?
}

impl Cuckoo {
    pub fn new(max_capacity: usize) -> Self {
        Cuckoo { remaining: max_capacity, filter: Some(CuckooFilter::with_capacity(max_capacity)) }
    }
}

impl Filter for Cuckoo {
    fn add(&mut self, element: &[u8]) -> Result<(), NippyJarError> {
        if self.remaining == 0 {
            return Err(NippyJarError::FilterMaxCapacity)
        }
        let filter = self.filter.as_mut().ok_or(NippyJarError::FilterCuckooNotLoaded)?;

        self.remaining -= 1;

        Ok(filter.add(element)?)
    }

    fn contains(&self, element: &[u8]) -> Result<bool, NippyJarError> {
        Ok(self.filter.as_ref().ok_or(NippyJarError::FilterCuckooNotLoaded)?.contains(element))
    }
}

impl std::fmt::Debug for Cuckoo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Cuckoo {{ remaining_elements: {:?}, filter.is_some(): {:?} }}",
            self.remaining,
            self.filter.is_some(),
        )
    }
}

impl PartialEq for Cuckoo {
    fn eq(&self, other: &Self) -> bool {
        self.remaining == other.remaining &&
            match (&self.filter, &other.filter) {
                (Some(_this), Some(_other)) => {
                    #[cfg(not(test))]
                    {
                        unimplemented!("No way to figure it out without exporting (expensive), so only allow direct comparison on a test")
                    }
                    #[cfg(test)]
                    {
                        let f1 = _this.export();
                        let f2 = _other.export();
                        return f1.length == f2.length && f1.values == f2.values
                    }
                }
                (None, None) => true,
                _ => false,
            }
    }
}

impl<'de> Deserialize<'de> for Cuckoo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let (remaining, exported): (usize, Option<ExportedCuckooFilter>) =
            Deserialize::deserialize(deserializer)?;

        Ok(Cuckoo { remaining, filter: exported.map(Into::into) })
    }
}

impl Serialize for Cuckoo {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        (self.remaining, self.filter.as_ref().map(|f| f.export())).serialize(serializer)
    }
}
