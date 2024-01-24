use alloy_primitives::{Address, Bytes};
use reth_rpc_types::PeerId;
use serde_derive::{Deserialize, Serialize};
use std::fmt;
// use itertools::free::join;
/// A set of validators
#[derive(Debug, Serialize, Deserialize)]
pub struct Validators {
    pub validators: Vec<PeerId>,
}

impl Validators {
    /// Create a new set of validators
    pub fn new(validators: Vec<PeerId>) -> Self {
        Self { validators }
    }

    pub fn len(&self) -> usize {
        self.validators.len()
    }

    pub fn index(&self, index: usize) -> PeerId {
        self.validators[index].clone()
    }

    /// Update the set of validators
    pub fn update(&mut self, validators: &Vec<PeerId>) {
        self.validators.clear();
        self.validators.clone_from(validators);
    }

    /// Obtain the ID for the primary node at the specified view
    pub fn get_primary_id(&self, view: u64) -> PeerId {
        let primary_index = (view as usize) % self.validators.len();
        self.validators[primary_index].clone()
    }

    /// Tell if this node is currently the primary
    pub fn is_primary(&self, id: PeerId, view: u64) -> bool {
        id == self.get_primary_id(view)
    }

    /// Tell if the set of validators is different
    pub fn is_different(&self, validators: &Vec<PeerId>) -> bool {
        let matching =
            self.validators.iter().zip(validators.iter()).filter(|&(a, b)| a == b).count();
        matching == self.validators.len() && matching == validators.len()
    }

    pub fn contains(&self, id: &PeerId) -> bool {
        self.validators.contains(id)
    }

    pub fn member_ids(&self) -> &Vec<PeerId> {
        &self.validators
    }
}

impl fmt::Display for Validators {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let s = self.validators.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", ");
        write!(f, "({})", s)
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn validators_is_defferent_test() {
        let a = "Hello";
        let b = "World";

        let matching = a.chars().zip(b.chars()).filter(|&(a, b)| a == b).count();
        println!("{}", matching);

        let a = [1, 2, 3, 4, 5];
        let b = [1, 1, 3, 3, 5];

        let matching = a.iter().zip(&b).filter(|&(a, b)| a == b).count();
        println!("{}", matching);
    }
}
