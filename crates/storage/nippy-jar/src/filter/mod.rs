use crate::NippyJarError;
use serde::{Deserialize, Serialize};

mod cuckoo;
pub use cuckoo::Cuckoo;

pub trait Filter {
    /// Add element to the inclusion list.
    fn add(&mut self, element: &[u8]) -> Result<(), NippyJarError>;

    /// Checks if the element belongs to the inclusion list. There might be false positives.
    fn contains(&self, element: &[u8]) -> Result<bool, NippyJarError>;

    /// Is the filter ready to be used.
    fn is_ready(&self) -> bool {
        true
    }

    /// Informs the filter algorithm that it was loaded from disk.
    fn was_loaded(&mut self) {
        unimplemented!()
    }

    /// Freezes the filter algorithm in use.
    fn freeze(&mut self) {
        unimplemented!()
    }
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

    fn is_ready(&self) -> bool {
        match self {
            Filters::Cuckoo(c) => c.is_ready(),
            Filters::Unused => todo!(),
        }
    }

    fn was_loaded(&mut self) {
        match self {
            Filters::Cuckoo(c) => c.was_loaded(),
            Filters::Unused => todo!(),
        }
    }

    fn freeze(&mut self) {
        match self {
            Filters::Cuckoo(c) => c.freeze(),
            Filters::Unused => todo!(),
        }
    }
}
