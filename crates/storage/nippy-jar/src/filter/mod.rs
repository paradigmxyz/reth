use crate::NippyJarError;
use serde::{Deserialize, Serialize};

mod cuckoo;
pub use cuckoo::Cuckoo;

pub trait Filter {
    /// Add element to the inclusion list.
    fn add(&mut self, element: &[u8]) -> Result<(), NippyJarError>;

    /// Checks if the element belongs to the inclusion list. There might be false positives.
    fn contains(&self, element: &[u8]) -> Result<bool, NippyJarError>;
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub enum Filters {
    Cuckoo(Cuckoo),
    // Avoids irrefutable let errors. Remove this after adding another one.
    Unused,
}

impl Filter for Filters {
    fn add(&mut self, element: &[u8]) -> Result<(), NippyJarError> {
        match self {
            Filters::Cuckoo(c) => c.add(element),
            Filters::Unused => todo!(),
        }
    }

    fn contains(&self, element: &[u8]) -> Result<bool, NippyJarError> {
        match self {
            Filters::Cuckoo(c) => c.contains(element),
            Filters::Unused => todo!(),
        }
    }
}
