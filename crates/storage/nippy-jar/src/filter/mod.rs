use serde::{Deserialize, Serialize};
use std::{io::Write, collections::hash_map::DefaultHasher};
use cuckoofilter::{self, ExportedCuckooFilter, CuckooFilter};
use crate::NippyJarError;

pub trait Filter {
    fn add(&mut self, element: &[u8]) -> Result<(), NippyJarError>;
    fn contains(&self, element: &[u8]) -> bool;
    fn is_ready(&self) -> bool {
        true
    }
    fn was_loaded(&mut self);
}

#[derive(Serialize, Deserialize, PartialEq)]
pub enum Filters {
    Cuckoo(Cuckoo),
    // Avoids irrefutable let errors. Remove this after adding another one.
    Unused,
}

#[derive(Serialize, Deserialize)]
pub struct Cuckoo {
    exported: Option<ExportedCuckooFilter>,
    remaining: usize,
    #[serde(skip)]
    filter: Option<CuckooFilter<DefaultHasher>> // TODO does it need an actual hasher?
}

impl Cuckoo {
    pub fn new(max_capacity: usize) -> Self {
        Cuckoo { exported: None, remaining: 0, filter: Some(CuckooFilter::with_capacity(max_capacity)) }
    }
}

impl Filter for Cuckoo {
    fn add(&mut self, element: &[u8]) -> Result<(), NippyJarError> {
        Ok(self.filter.as_mut().expect("exists").add(element)?)
    }

    fn contains(&self, element: &[u8]) -> bool {
        self.filter.as_ref().expect("exists").contains(element)
    }

    fn is_ready(&self) -> bool {
        self.filter.is_some()
    }

    fn was_loaded(&mut self) {
        self.filter = self.exported.take().map(Into::into);
    }
}

