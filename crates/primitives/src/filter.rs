#![allow(missing_docs)]

use crate::{
    bloom::{Bloom, Input},
    keccak256, Address, BlockNumberOrTag, Log, H160, H256, U256, U64,
};
use serde::{
    de::{DeserializeOwned, MapAccess, Visitor},
    ser::SerializeStruct,
    Deserialize, Deserializer, Serialize, Serializer,
};
use std::ops::{Range, RangeFrom, RangeTo};

pub type BloomFilter = Vec<Option<Bloom>>;

/// A single topic
pub type Topic = ValueOrArray<Option<H256>>;

/// Represents the target range of blocks for the filter
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum FilterBlockOption {
    Range { from_block: Option<BlockNumberOrTag>, to_block: Option<BlockNumberOrTag> },
    AtBlockHash(H256),
}

impl FilterBlockOption {
    pub fn get_to_block(&self) -> Option<&BlockNumberOrTag> {
        match self {
            FilterBlockOption::Range { to_block, .. } => to_block.as_ref(),
            FilterBlockOption::AtBlockHash(_) => None,
        }
    }

    pub fn get_from_block(&self) -> Option<&BlockNumberOrTag> {
        match self {
            FilterBlockOption::Range { from_block, .. } => from_block.as_ref(),
            FilterBlockOption::AtBlockHash(_) => None,
        }
    }
}

impl From<BlockNumberOrTag> for FilterBlockOption {
    fn from(block: BlockNumberOrTag) -> Self {
        let block = Some(block);
        FilterBlockOption::Range { from_block: block, to_block: block }
    }
}

impl From<U64> for FilterBlockOption {
    fn from(block: U64) -> Self {
        BlockNumberOrTag::from(block).into()
    }
}

impl From<u64> for FilterBlockOption {
    fn from(block: u64) -> Self {
        BlockNumberOrTag::from(block).into()
    }
}

impl<T: Into<BlockNumberOrTag>> From<Range<T>> for FilterBlockOption {
    fn from(r: Range<T>) -> Self {
        let from_block = Some(r.start.into());
        let to_block = Some(r.end.into());
        FilterBlockOption::Range { from_block, to_block }
    }
}

impl<T: Into<BlockNumberOrTag>> From<RangeTo<T>> for FilterBlockOption {
    fn from(r: RangeTo<T>) -> Self {
        let to_block = Some(r.end.into());
        FilterBlockOption::Range { from_block: Some(BlockNumberOrTag::Earliest), to_block }
    }
}

impl<T: Into<BlockNumberOrTag>> From<RangeFrom<T>> for FilterBlockOption {
    fn from(r: RangeFrom<T>) -> Self {
        let from_block = Some(r.start.into());
        FilterBlockOption::Range { from_block, to_block: Some(BlockNumberOrTag::Latest) }
    }
}

impl From<H256> for FilterBlockOption {
    fn from(hash: H256) -> Self {
        FilterBlockOption::AtBlockHash(hash)
    }
}

impl Default for FilterBlockOption {
    fn default() -> Self {
        FilterBlockOption::Range { from_block: None, to_block: None }
    }
}

impl FilterBlockOption {
    #[must_use]
    pub fn set_from_block(&self, block: BlockNumberOrTag) -> Self {
        let to_block =
            if let FilterBlockOption::Range { to_block, .. } = self { *to_block } else { None };

        FilterBlockOption::Range { from_block: Some(block), to_block }
    }

    #[must_use]
    pub fn set_to_block(&self, block: BlockNumberOrTag) -> Self {
        let from_block =
            if let FilterBlockOption::Range { from_block, .. } = self { *from_block } else { None };

        FilterBlockOption::Range { from_block, to_block: Some(block) }
    }

    #[must_use]
    pub fn set_hash(&self, hash: H256) -> Self {
        FilterBlockOption::AtBlockHash(hash)
    }
}

/// Filter for
#[derive(Default, Debug, PartialEq, Eq, Clone, Hash)]
pub struct Filter {
    /// Filter block options, specifying on which blocks the filter should
    /// match.
    // https://eips.ethereum.org/EIPS/eip-234
    pub block_option: FilterBlockOption,
    /// Address
    pub address: Option<ValueOrArray<Address>>,
    /// Topics (maxmimum of 4)
    pub topics: [Option<Topic>; 4],
}

impl Filter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the inner filter object
    ///
    /// *NOTE:* ranges are always inclusive
    ///
    /// # Examples
    ///
    /// Match only a specific block
    ///
    /// ```rust
    /// # use reth_primitives::filter::Filter;
    /// # fn main() {
    /// let filter = Filter::new().select(69u64);
    /// # }
    /// ```
    /// This is the same as `Filter::new().from_block(1337u64).to_block(1337u64)`
    ///
    /// Match the latest block only
    ///
    /// ```rust
    /// # use reth_primitives::{filter::Filter, BlockNumberOrTag};
    /// # fn main() {
    /// let filter = Filter::new().select(BlockNumberOrTag::Latest);
    /// # }
    /// ```
    ///
    /// Match a block by its hash
    ///
    /// ```rust
    /// # use reth_primitives::{filter::Filter, H256};
    /// # fn main() {
    /// let filter = Filter::new().select(H256::zero());
    /// # }
    /// ```
    /// This is the same as `at_block_hash`
    ///
    /// Match a range of blocks
    ///
    /// ```rust
    /// # use reth_primitives::filter::Filter;
    /// # fn main() {
    /// let filter = Filter::new().select(0u64..100u64);
    /// # }
    /// ```
    ///
    /// Match all blocks in range `(1337..BlockNumberOrTag::Latest)`
    ///
    /// ```rust
    /// # use reth_primitives::filter::Filter;
    /// # fn main() {
    /// let filter = Filter::new().select(1337u64..);
    /// # }
    /// ```
    ///
    /// Match all blocks in range `(BlockNumberOrTag::Earliest..1337)`
    ///
    /// ```rust
    /// # use reth_primitives::filter::Filter;
    /// # fn main() {
    /// let filter = Filter::new().select(..1337u64);
    /// # }
    /// ```
    #[must_use]
    pub fn select(mut self, filter: impl Into<FilterBlockOption>) -> Self {
        self.block_option = filter.into();
        self
    }

    #[allow(clippy::wrong_self_convention)]
    #[must_use]
    pub fn from_block<T: Into<BlockNumberOrTag>>(mut self, block: T) -> Self {
        self.block_option = self.block_option.set_from_block(block.into());
        self
    }

    #[allow(clippy::wrong_self_convention)]
    #[must_use]
    pub fn to_block<T: Into<BlockNumberOrTag>>(mut self, block: T) -> Self {
        self.block_option = self.block_option.set_to_block(block.into());
        self
    }

    #[allow(clippy::wrong_self_convention)]
    #[must_use]
    pub fn at_block_hash<T: Into<H256>>(mut self, hash: T) -> Self {
        self.block_option = self.block_option.set_hash(hash.into());
        self
    }
    /// Sets the inner filter object
    ///
    /// *NOTE:* ranges are always inclusive
    ///
    /// # Examples
    ///
    /// Match only a specific address `("0xAc4b3DacB91461209Ae9d41EC517c2B9Cb1B7DAF")`
    ///
    /// ```rust
    /// # use reth_primitives::{Address, filter::Filter};
    /// # fn main() {
    /// let filter = Filter::new().address("0xAc4b3DacB91461209Ae9d41EC517c2B9Cb1B7DAF".parse::<Address>().unwrap());
    /// # }
    /// ```
    ///
    /// Match all addresses in array `(vec!["0xAc4b3DacB91461209Ae9d41EC517c2B9Cb1B7DAF",
    /// "0x8ad599c3A0ff1De082011EFDDc58f1908eb6e6D8"])`
    ///
    /// ```rust
    /// # use reth_primitives::{filter::Filter, Address};
    /// # fn main() {
    /// let addresses = vec!["0xAc4b3DacB91461209Ae9d41EC517c2B9Cb1B7DAF".parse::<Address>().unwrap(),"0x8ad599c3A0ff1De082011EFDDc58f1908eb6e6D8".parse::<Address>().unwrap()];
    /// let filter = Filter::new().address(addresses);
    /// # }
    /// ```
    #[must_use]
    pub fn address<T: Into<ValueOrArray<Address>>>(mut self, address: T) -> Self {
        self.address = Some(address.into());
        self
    }

    /// Given the event signature in string form, it hashes it and adds it to the topics to monitor
    #[must_use]
    pub fn event(self, event_name: &str) -> Self {
        let hash = keccak256(event_name.as_bytes());
        self.topic0(hash)
    }

    /// Hashes all event signatures and sets them as array to topic0
    #[must_use]
    pub fn events(self, events: impl IntoIterator<Item = impl AsRef<[u8]>>) -> Self {
        let events = events.into_iter().map(|e| keccak256(e.as_ref())).collect::<Vec<_>>();
        self.topic0(events)
    }

    /// Sets topic0 (the event name for non-anonymous events)
    #[must_use]
    pub fn topic0<T: Into<Topic>>(mut self, topic: T) -> Self {
        self.topics[0] = Some(topic.into());
        self
    }

    /// Sets the 1st indexed topic
    #[must_use]
    pub fn topic1<T: Into<Topic>>(mut self, topic: T) -> Self {
        self.topics[1] = Some(topic.into());
        self
    }

    /// Sets the 2nd indexed topic
    #[must_use]
    pub fn topic2<T: Into<Topic>>(mut self, topic: T) -> Self {
        self.topics[2] = Some(topic.into());
        self
    }

    /// Sets the 3rd indexed topic
    #[must_use]
    pub fn topic3<T: Into<Topic>>(mut self, topic: T) -> Self {
        self.topics[3] = Some(topic.into());
        self
    }

    pub fn is_paginatable(&self) -> bool {
        self.get_from_block().is_some()
    }

    /// Returns the numeric value of the `toBlock` field
    pub fn get_to_block(&self) -> Option<u64> {
        self.block_option.get_to_block().and_then(|b| b.as_number())
    }

    /// Returns the numeric value of the `fromBlock` field
    pub fn get_from_block(&self) -> Option<u64> {
        self.block_option.get_from_block().and_then(|b| b.as_number())
    }

    /// Returns the numeric value of the `fromBlock` field
    pub fn get_block_hash(&self) -> Option<H256> {
        match self.block_option {
            FilterBlockOption::AtBlockHash(hash) => Some(hash),
            FilterBlockOption::Range { .. } => None,
        }
    }

    /// Flattens the topics using the cartesian product
    fn flatten(&self) -> Vec<ValueOrArray<Option<H256>>> {
        fn cartesian(lists: &[Vec<Option<H256>>]) -> Vec<Vec<Option<H256>>> {
            let mut res = Vec::new();
            let mut list_iter = lists.iter();
            if let Some(first_list) = list_iter.next() {
                for &i in first_list {
                    res.push(vec![i]);
                }
            }
            for l in list_iter {
                let mut tmp = Vec::new();
                for r in res {
                    for &el in l {
                        let mut tmp_el = r.clone();
                        tmp_el.push(el);
                        tmp.push(tmp_el);
                    }
                }
                res = tmp;
            }
            res
        }
        let mut out = Vec::new();
        let mut tmp = Vec::new();
        for v in self.topics.iter() {
            let v = if let Some(v) = v {
                match v {
                    ValueOrArray::Value(s) => {
                        vec![*s]
                    }
                    ValueOrArray::Array(s) => s.clone(),
                }
            } else {
                vec![None]
            };
            tmp.push(v);
        }
        for v in cartesian(&tmp) {
            out.push(ValueOrArray::Array(v));
        }
        out
    }

    /// Returns an iterator over all existing topics
    pub fn topics(&self) -> impl Iterator<Item = &Topic> + '_ {
        self.topics.iter().flatten()
    }

    /// Returns true if at least one topic is set
    pub fn has_topics(&self) -> bool {
        self.topics.iter().any(|t| t.is_some())
    }
}

impl Serialize for Filter {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut s = serializer.serialize_struct("Filter", 5)?;
        match self.block_option {
            FilterBlockOption::Range { from_block, to_block } => {
                if let Some(ref from_block) = from_block {
                    s.serialize_field("fromBlock", from_block)?;
                }

                if let Some(ref to_block) = to_block {
                    s.serialize_field("toBlock", to_block)?;
                }
            }

            FilterBlockOption::AtBlockHash(ref h) => s.serialize_field("blockHash", h)?,
        }

        if let Some(ref address) = self.address {
            s.serialize_field("address", address)?;
        }

        let mut filtered_topics = Vec::new();
        for i in 0..4 {
            if self.topics[i].is_some() {
                filtered_topics.push(&self.topics[i]);
            } else {
                // TODO: This can be optimized
                if self.topics[i + 1..].iter().any(|x| x.is_some()) {
                    filtered_topics.push(&None);
                }
            }
        }
        s.serialize_field("topics", &filtered_topics)?;

        s.end()
    }
}

impl<'de> Deserialize<'de> for Filter {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct FilterVisitor;

        impl<'de> Visitor<'de> for FilterVisitor {
            type Value = Filter;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("Filter object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut from_block: Option<Option<BlockNumberOrTag>> = None;
                let mut to_block: Option<Option<BlockNumberOrTag>> = None;
                let mut block_hash: Option<Option<H256>> = None;
                let mut address: Option<Option<ValueOrArray<Address>>> = None;
                let mut topics: Option<Option<Vec<Option<Topic>>>> = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "fromBlock" => {
                            if from_block.is_some() {
                                return Err(serde::de::Error::duplicate_field("fromBlock"))
                            }
                            if block_hash.is_some() {
                                return Err(serde::de::Error::custom(
                                    "fromBlock not allowed with blockHash",
                                ))
                            }
                            from_block = Some(map.next_value()?)
                        }
                        "toBlock" => {
                            if to_block.is_some() {
                                return Err(serde::de::Error::duplicate_field("toBlock"))
                            }
                            if block_hash.is_some() {
                                return Err(serde::de::Error::custom(
                                    "toBlock not allowed with blockHash",
                                ))
                            }
                            to_block = Some(map.next_value()?)
                        }
                        "blockHash" => {
                            if block_hash.is_some() {
                                return Err(serde::de::Error::duplicate_field("blockHash"))
                            }
                            if from_block.is_some() || to_block.is_some() {
                                return Err(serde::de::Error::custom(
                                    "fromBlock,toBlock not allowed with blockHash",
                                ))
                            }
                            block_hash = Some(map.next_value()?)
                        }
                        "address" => {
                            if address.is_some() {
                                return Err(serde::de::Error::duplicate_field("address"))
                            }
                            address = Some(map.next_value()?)
                        }
                        "topics" => {
                            if topics.is_some() {
                                return Err(serde::de::Error::duplicate_field("topics"))
                            }
                            topics = Some(map.next_value()?)
                        }

                        key => {
                            return Err(serde::de::Error::unknown_field(
                                key,
                                &["fromBlock", "toBlock", "address", "topics", "blockHash"],
                            ))
                        }
                    }
                }

                let from_block = from_block.unwrap_or_default();
                let to_block = to_block.unwrap_or_default();
                let block_hash = block_hash.unwrap_or_default();
                let address = address.unwrap_or_default();
                let topics_vec = topics.flatten().unwrap_or_default();

                // maximum allowed filter len
                if topics_vec.len() > 4 {
                    return Err(serde::de::Error::custom("exceeded maximum topics len"))
                }
                let mut topics: [Option<Topic>; 4] = [None, None, None, None];
                for (idx, topic) in topics_vec.into_iter().enumerate() {
                    topics[idx] = topic;
                }

                let block_option = if let Some(block_hash) = block_hash {
                    FilterBlockOption::AtBlockHash(block_hash)
                } else {
                    FilterBlockOption::Range { from_block, to_block }
                };

                Ok(Filter { block_option, address, topics })
            }
        }

        deserializer.deserialize_any(FilterVisitor)
    }
}

/// Union type for representing a single value or a vector of values inside a filter
#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub enum ValueOrArray<T> {
    /// A single value
    Value(T),
    /// A vector of values
    Array(Vec<T>),
}

impl From<H160> for ValueOrArray<H160> {
    fn from(src: H160) -> Self {
        ValueOrArray::Value(src)
    }
}

impl From<Vec<H160>> for ValueOrArray<H160> {
    fn from(src: Vec<H160>) -> Self {
        ValueOrArray::Array(src)
    }
}

impl From<H256> for Topic {
    fn from(src: H256) -> Self {
        ValueOrArray::Value(Some(src))
    }
}

impl From<Vec<H256>> for ValueOrArray<H256> {
    fn from(src: Vec<H256>) -> Self {
        ValueOrArray::Array(src)
    }
}

impl From<ValueOrArray<H256>> for Topic {
    fn from(src: ValueOrArray<H256>) -> Self {
        match src {
            ValueOrArray::Value(val) => ValueOrArray::Value(Some(val)),
            ValueOrArray::Array(arr) => arr.into(),
        }
    }
}

impl<I: Into<H256>> From<Vec<I>> for Topic {
    fn from(src: Vec<I>) -> Self {
        ValueOrArray::Array(src.into_iter().map(Into::into).map(Some).collect())
    }
}

impl From<Address> for Topic {
    fn from(src: Address) -> Self {
        let mut bytes = [0; 32];
        bytes[12..32].copy_from_slice(src.as_bytes());
        ValueOrArray::Value(Some(H256::from(bytes)))
    }
}

impl From<U256> for Topic {
    fn from(src: U256) -> Self {
        ValueOrArray::Value(Some(src.into()))
    }
}

impl<T> Serialize for ValueOrArray<T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            ValueOrArray::Value(inner) => inner.serialize(serializer),
            ValueOrArray::Array(inner) => inner.serialize(serializer),
        }
    }
}

impl<'a, T> Deserialize<'a> for ValueOrArray<T>
where
    T: DeserializeOwned,
{
    fn deserialize<D>(deserializer: D) -> Result<ValueOrArray<T>, D::Error>
    where
        D: Deserializer<'a>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        if value.is_null() {
            return Ok(ValueOrArray::Array(Vec::new()))
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Variadic<T> {
            Value(T),
            Array(Vec<T>),
        }

        match serde_json::from_value::<Variadic<T>>(value).map_err(|err| {
            serde::de::Error::custom(format!("Invalid variadic value or array type: {err}"))
        })? {
            Variadic::Value(val) => Ok(ValueOrArray::Value(val)),
            Variadic::Array(arr) => Ok(ValueOrArray::Array(arr)),
        }
    }
}

/// Support for matching [Filter]s
#[derive(Debug, Default)]
pub struct FilteredParams {
    pub filter: Option<Filter>,
    pub flat_topics: Vec<ValueOrArray<Option<H256>>>,
}

impl FilteredParams {
    pub fn new(filter: Option<Filter>) -> Self {
        if let Some(filter) = filter {
            let flat_topics = filter.flatten();
            FilteredParams { filter: Some(filter), flat_topics }
        } else {
            Default::default()
        }
    }

    /// Returns the [BloomFilter] for the given address
    pub fn address_filter(address: &Option<ValueOrArray<Address>>) -> BloomFilter {
        address.as_ref().map(address_to_bloom_filter).unwrap_or_default()
    }

    /// Returns the [BloomFilter] for the given topics
    pub fn topics_filter(topics: &Option<Vec<ValueOrArray<Option<H256>>>>) -> Vec<BloomFilter> {
        let mut output = Vec::new();
        if let Some(topics) = topics {
            output.extend(topics.iter().map(topics_to_bloom_filter));
        }
        output
    }

    /// Returns `true` if the bloom matches the topics
    pub fn matches_topics(bloom: Bloom, topic_filters: &[BloomFilter]) -> bool {
        if topic_filters.is_empty() {
            return true
        }

        // returns true if a filter matches
        for filter in topic_filters.iter() {
            let mut is_match = false;
            for maybe_bloom in filter {
                is_match = maybe_bloom.as_ref().map(|b| bloom.contains_bloom(b)).unwrap_or(true);
                if !is_match {
                    break
                }
            }
            if is_match {
                return true
            }
        }
        false
    }

    /// Returns `true` if the bloom contains the address
    pub fn matches_address(bloom: Bloom, address_filter: &BloomFilter) -> bool {
        if address_filter.is_empty() {
            return true
        } else {
            for maybe_bloom in address_filter {
                if maybe_bloom.as_ref().map(|b| bloom.contains_bloom(b)).unwrap_or(true) {
                    return true
                }
            }
        }
        false
    }

    /// Replace None values - aka wildcards - for the log input value in that position.
    pub fn replace(&self, log: &Log, topic: Topic) -> Option<Vec<H256>> {
        let mut out: Vec<H256> = Vec::new();
        match topic {
            ValueOrArray::Value(value) => {
                if let Some(value) = value {
                    out.push(value);
                }
            }
            ValueOrArray::Array(value) => {
                for (k, v) in value.into_iter().enumerate() {
                    if let Some(v) = v {
                        out.push(v);
                    } else {
                        out.push(log.topics[k]);
                    }
                }
            }
        };
        if out.is_empty() {
            return None
        }
        Some(out)
    }

    pub fn filter_block_range(&self, block_number: u64) -> bool {
        if self.filter.is_none() {
            return true
        }
        let filter = self.filter.as_ref().unwrap();
        let mut res = true;

        if let Some(BlockNumberOrTag::Number(num)) = filter.block_option.get_from_block() {
            if *num > block_number {
                res = false;
            }
        }

        if let Some(to) = filter.block_option.get_to_block() {
            match to {
                BlockNumberOrTag::Number(num) => {
                    if *num < block_number {
                        res = false;
                    }
                }
                BlockNumberOrTag::Earliest => {
                    res = false;
                }
                _ => {}
            }
        }
        res
    }

    pub fn filter_block_hash(&self, block_hash: H256) -> bool {
        if let Some(h) = self.filter.as_ref().and_then(|f| f.get_block_hash()) {
            if h != block_hash {
                return false
            }
        }
        true
    }

    pub fn filter_address(&self, log: &Log) -> bool {
        if let Some(input_address) = &self.filter.as_ref().and_then(|f| f.address.clone()) {
            match input_address {
                ValueOrArray::Value(x) => {
                    if log.address != *x {
                        return false
                    }
                }
                ValueOrArray::Array(x) => {
                    if x.is_empty() {
                        return true
                    }
                    if !x.contains(&log.address) {
                        return false
                    }
                }
            }
        }
        true
    }

    pub fn filter_topics(&self, log: &Log) -> bool {
        let mut out: bool = true;
        for topic in self.flat_topics.iter().cloned() {
            match topic {
                ValueOrArray::Value(single) => {
                    if let Some(single) = single {
                        if !log.topics.starts_with(&[single]) {
                            out = false;
                        }
                    }
                }
                ValueOrArray::Array(multi) => {
                    if multi.is_empty() {
                        out = true;
                        continue
                    }
                    // Shrink the topics until the last item is Some.
                    let mut new_multi = multi;
                    while new_multi.iter().last().unwrap_or(&Some(H256::default())).is_none() {
                        new_multi.pop();
                    }
                    // We can discard right away any logs with lesser topics than the filter.
                    if new_multi.len() > log.topics.len() {
                        out = false;
                        break
                    }
                    let replaced: Option<Vec<H256>> =
                        self.replace(log, ValueOrArray::Array(new_multi));
                    if let Some(replaced) = replaced {
                        out = false;
                        if log.topics.starts_with(&replaced[..]) {
                            out = true;
                            break
                        }
                    }
                }
            }
        }
        out
    }
}

fn topics_to_bloom_filter(topics: &ValueOrArray<Option<H256>>) -> BloomFilter {
    let mut blooms = BloomFilter::new();
    match topics {
        ValueOrArray::Value(topic) => {
            if let Some(topic) = topic {
                let bloom: Bloom = Input::Raw(topic.as_ref()).into();
                blooms.push(Some(bloom));
            } else {
                blooms.push(None);
            }
        }
        ValueOrArray::Array(topics) => {
            if topics.is_empty() {
                blooms.push(None);
            } else {
                for topic in topics.iter() {
                    if let Some(topic) = topic {
                        let bloom: Bloom = Input::Raw(topic.as_ref()).into();
                        blooms.push(Some(bloom));
                    } else {
                        blooms.push(None);
                    }
                }
            }
        }
    }
    blooms
}

fn address_to_bloom_filter(address: &ValueOrArray<Address>) -> BloomFilter {
    let mut blooms = BloomFilter::new();
    match address {
        ValueOrArray::Value(address) => {
            let bloom: Bloom = Input::Raw(address.as_ref()).into();
            blooms.push(Some(bloom))
        }
        ValueOrArray::Array(addresses) => {
            if addresses.is_empty() {
                blooms.push(None);
            } else {
                for address in addresses.iter() {
                    let bloom: Bloom = Input::Raw(address.as_ref()).into();
                    blooms.push(Some(bloom));
                }
            }
        }
    }
    blooms
}

#[cfg(test)]
mod tests {
    use super::*;
    use ethers_core::utils::serialize;
    use serde_json::json;

    #[test]
    fn can_serde_value_or_array() {
        #[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
        struct Item {
            value: ValueOrArray<U256>,
        }

        let item = Item { value: ValueOrArray::Value(U256::from(1u64)) };
        let json = serde_json::to_value(item.clone()).unwrap();
        let deserialized: Item = serde_json::from_value(json).unwrap();
        assert_eq!(item, deserialized);

        let item = Item { value: ValueOrArray::Array(vec![U256::from(1u64), U256::ZERO]) };
        let json = serde_json::to_value(item.clone()).unwrap();
        let deserialized: Item = serde_json::from_value(json).unwrap();
        assert_eq!(item, deserialized);
    }

    #[test]
    fn filter_serialization_test() {
        let t1 = "9729a6fbefefc8f6005933898b13dc45c3a2c8b7".parse::<Address>().unwrap();
        let t2 = H256::from([0; 32]);
        let t3 = U256::from(123);

        let t1_padded = H256::from(t1);
        let t3_padded = H256::from({
            let mut x = [0; 32];
            x[31] = 123;
            x
        });

        let event = "ValueChanged(address,string,string)";
        let t0 = keccak256(event.as_bytes());
        let addr: Address = "f817796F60D268A36a57b8D2dF1B97B14C0D0E1d".parse().unwrap();
        let filter = Filter::new();

        let ser = serialize(&filter);
        assert_eq!(ser, json!({ "topics": [] }));

        let filter = filter.address(ValueOrArray::Value(addr));

        let ser = serialize(&filter);
        assert_eq!(ser, json!({"address" : addr, "topics": []}));

        let filter = filter.event(event);

        // 0
        let ser = serialize(&filter);
        assert_eq!(ser, json!({ "address" : addr, "topics": [t0]}));

        // 1
        let ser = serialize(&filter.clone().topic1(t1));
        assert_eq!(ser, json!({ "address" : addr, "topics": [t0, t1_padded]}));

        // 2
        let ser = serialize(&filter.clone().topic2(t2));
        assert_eq!(ser, json!({ "address" : addr, "topics": [t0, null, t2]}));

        // 3
        let ser = serialize(&filter.clone().topic3(t3));
        assert_eq!(ser, json!({ "address" : addr, "topics": [t0, null, null, t3_padded]}));

        // 1 & 2
        let ser = serialize(&filter.clone().topic1(t1).topic2(t2));
        assert_eq!(ser, json!({ "address" : addr, "topics": [t0, t1_padded, t2]}));

        // 1 & 3
        let ser = serialize(&filter.clone().topic1(t1).topic3(t3));
        assert_eq!(ser, json!({ "address" : addr, "topics": [t0, t1_padded, null, t3_padded]}));

        // 2 & 3
        let ser = serialize(&filter.clone().topic2(t2).topic3(t3));
        assert_eq!(ser, json!({ "address" : addr, "topics": [t0, null, t2, t3_padded]}));

        // 1 & 2 & 3
        let ser = serialize(&filter.topic1(t1).topic2(t2).topic3(t3));
        assert_eq!(ser, json!({ "address" : addr, "topics": [t0, t1_padded, t2, t3_padded]}));
    }

    fn build_bloom(address: Address, topic1: H256, topic2: H256) -> Bloom {
        let mut block_bloom = Bloom::default();
        block_bloom.accrue(Input::Raw(&address[..]));
        block_bloom.accrue(Input::Raw(&topic1[..]));
        block_bloom.accrue(Input::Raw(&topic2[..]));
        block_bloom
    }

    fn topic_filter(
        topic1: H256,
        topic2: H256,
        topic3: H256,
    ) -> (Filter, Option<Vec<ValueOrArray<Option<H256>>>>) {
        let filter = Filter {
            block_option: Default::default(),
            address: None,
            topics: [
                Some(ValueOrArray::Value(Some(topic1))),
                Some(ValueOrArray::Array(vec![Some(topic2), Some(topic3)])),
                None,
                None,
            ],
        };
        let filtered_params = FilteredParams::new(Some(filter.clone()));

        (filter, Some(filtered_params.flat_topics))
    }

    #[test]
    fn can_detect_different_topics() {
        let topic1 = H256::random();
        let topic2 = H256::random();
        let topic3 = H256::random();

        let (_, topics) = topic_filter(topic1, topic2, topic3);
        let topics_bloom = FilteredParams::topics_filter(&topics);
        assert!(!FilteredParams::matches_topics(
            build_bloom(Address::random(), H256::random(), H256::random()),
            &topics_bloom
        ));
    }

    #[test]
    fn can_match_topic() {
        let topic1 = H256::random();
        let topic2 = H256::random();
        let topic3 = H256::random();

        let (_, topics) = topic_filter(topic1, topic2, topic3);
        let _topics_bloom = FilteredParams::topics_filter(&topics);

        let topics_bloom = FilteredParams::topics_filter(&topics);
        assert!(FilteredParams::matches_topics(
            build_bloom(Address::random(), topic1, topic2),
            &topics_bloom
        ));
    }

    #[test]
    fn can_match_empty_topics() {
        let filter =
            Filter { block_option: Default::default(), address: None, topics: Default::default() };

        let filtered_params = FilteredParams::new(Some(filter));
        let topics = Some(filtered_params.flat_topics);

        let topics_bloom = FilteredParams::topics_filter(&topics);
        assert!(FilteredParams::matches_topics(
            build_bloom(Address::random(), H256::random(), H256::random()),
            &topics_bloom
        ));
    }

    #[test]
    fn can_match_address_and_topics() {
        let rng_address = Address::random();
        let topic1 = H256::random();
        let topic2 = H256::random();
        let topic3 = H256::random();

        let filter = Filter {
            block_option: Default::default(),
            address: Some(ValueOrArray::Value(rng_address)),
            topics: [
                Some(ValueOrArray::Value(Some(topic1))),
                Some(ValueOrArray::Array(vec![Some(topic2), Some(topic3)])),
                None,
                None,
            ],
        };
        let filtered_params = FilteredParams::new(Some(filter.clone()));
        let topics = Some(filtered_params.flat_topics);
        let address_filter = FilteredParams::address_filter(&filter.address);
        let topics_filter = FilteredParams::topics_filter(&topics);
        assert!(
            FilteredParams::matches_address(
                build_bloom(rng_address, topic1, topic2),
                &address_filter
            ) && FilteredParams::matches_topics(
                build_bloom(rng_address, topic1, topic2),
                &topics_filter
            )
        );
    }

    #[test]
    fn can_match_topics_wildcard() {
        let topic1 = H256::random();
        let topic2 = H256::random();
        let topic3 = H256::random();

        let filter = Filter {
            block_option: Default::default(),
            address: None,
            topics: [None, Some(ValueOrArray::Array(vec![Some(topic2), Some(topic3)])), None, None],
        };
        let filtered_params = FilteredParams::new(Some(filter));
        let topics = Some(filtered_params.flat_topics);
        let topics_bloom = FilteredParams::topics_filter(&topics);
        assert!(FilteredParams::matches_topics(
            build_bloom(Address::random(), topic1, topic2),
            &topics_bloom
        ));
    }

    #[test]
    fn can_match_topics_wildcard_mismatch() {
        let filter = Filter {
            block_option: Default::default(),
            address: None,
            topics: [
                None,
                Some(ValueOrArray::Array(vec![Some(H256::random()), Some(H256::random())])),
                None,
                None,
            ],
        };
        let filtered_params = FilteredParams::new(Some(filter));
        let topics_input = Some(filtered_params.flat_topics);
        let topics_bloom = FilteredParams::topics_filter(&topics_input);
        assert!(!FilteredParams::matches_topics(
            build_bloom(Address::random(), H256::random(), H256::random()),
            &topics_bloom
        ));
    }

    #[test]
    fn can_match_address_filter() {
        let rng_address = Address::random();
        let filter = Filter {
            block_option: Default::default(),
            address: Some(ValueOrArray::Value(rng_address)),
            topics: Default::default(),
        };
        let address_bloom = FilteredParams::address_filter(&filter.address);
        assert!(FilteredParams::matches_address(
            build_bloom(rng_address, H256::random(), H256::random(),),
            &address_bloom
        ));
    }

    #[test]
    fn can_detect_different_address() {
        let bloom_address = Address::random();
        let rng_address = Address::random();
        let filter = Filter {
            block_option: Default::default(),
            address: Some(ValueOrArray::Value(rng_address)),
            topics: Default::default(),
        };
        let address_bloom = FilteredParams::address_filter(&filter.address);
        assert!(!FilteredParams::matches_address(
            build_bloom(bloom_address, H256::random(), H256::random(),),
            &address_bloom
        ));
    }

    #[test]
    fn can_convert_to_ethers_filter() {
        let json = json!(
                    {
          "fromBlock": "0x429d3b",
          "toBlock": "0x429d3b",
          "address": "0xb59f67a8bff5d8cd03f6ac17265c550ed8f33907",
          "topics": [
          "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef",
          "0x00000000000000000000000000b46c2526e227482e2ebb8f4c69e4674d262e75",
          "0x00000000000000000000000054a2d42a40f51259dedd1978f6c118a0f0eff078"
          ]
        }
            );

        let filter: Filter = serde_json::from_value(json).unwrap();
        assert_eq!(
            filter,
            Filter {
                block_option: FilterBlockOption::Range {
                    from_block: Some(4365627u64.into()),
                    to_block: Some(4365627u64.into()),
                },
                address: Some(ValueOrArray::Value(
                    "0xb59f67a8bff5d8cd03f6ac17265c550ed8f33907".parse().unwrap()
                )),
                topics: [
                    Some(ValueOrArray::Value(Some(
                        "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"
                            .parse()
                            .unwrap(),
                    ))),
                    Some(ValueOrArray::Value(Some(
                        "0x00000000000000000000000000b46c2526e227482e2ebb8f4c69e4674d262e75"
                            .parse()
                            .unwrap(),
                    ))),
                    Some(ValueOrArray::Value(Some(
                        "0x00000000000000000000000054a2d42a40f51259dedd1978f6c118a0f0eff078"
                            .parse()
                            .unwrap(),
                    ))),
                    None,
                ],
            }
        );
    }

    #[test]
    fn can_convert_to_ethers_filter_with_null_fields() {
        let json = json!(
                    {
          "fromBlock": "0x429d3b",
          "toBlock": "0x429d3b",
          "address": null,
          "topics": null
        }
            );

        let filter: Filter = serde_json::from_value(json).unwrap();
        assert_eq!(
            filter,
            Filter {
                block_option: FilterBlockOption::Range {
                    from_block: Some(4365627u64.into()),
                    to_block: Some(4365627u64.into()),
                },
                address: None,
                topics: [None, None, None, None,],
            }
        );
    }
}
