use reth_primitives::{B256, U256};
use serde::{
    de::{Error, SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize, Serializer,
};
use std::fmt;

/// The result of an `eth_getWork` request
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Work {
    /// The proof-of-work hash.
    pub pow_hash: B256,
    /// The seed hash.
    pub seed_hash: B256,
    /// The target.
    pub target: B256,
    /// The block number: this isn't always stored.
    pub number: Option<u64>,
}

impl Serialize for Work {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self.number.as_ref() {
            Some(num) => {
                (&self.pow_hash, &self.seed_hash, &self.target, U256::from(*num)).serialize(s)
            }
            None => (&self.pow_hash, &self.seed_hash, &self.target).serialize(s),
        }
    }
}

impl<'a> Deserialize<'a> for Work {
    fn deserialize<D>(deserializer: D) -> Result<Work, D::Error>
    where
        D: Deserializer<'a>,
    {
        struct WorkVisitor;

        impl<'a> Visitor<'a> for WorkVisitor {
            type Value = Work;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "Work object")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'a>,
            {
                let pow_hash = seq
                    .next_element::<B256>()?
                    .ok_or_else(|| A::Error::custom("missing pow hash"))?;
                let seed_hash = seq
                    .next_element::<B256>()?
                    .ok_or_else(|| A::Error::custom("missing seed hash"))?;
                let target = seq
                    .next_element::<B256>()?
                    .ok_or_else(|| A::Error::custom("missing target"))?;
                let number = seq.next_element::<u64>()?;
                Ok(Work { pow_hash, seed_hash, target, number })
            }
        }

        deserializer.deserialize_any(WorkVisitor)
    }
}
