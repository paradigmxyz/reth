use std::{error::Error, fmt};

use serde::Deserialize;

use crate::SCHEMA_ID;
const ROOT_TYPE: &str = "ReceiptsSSZ";

#[derive(Debug)]
pub enum VectorError {
    Json(serde_json::Error),
    Invalid(String),
}

impl fmt::Display for VectorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "invalid JSON: {error}"),
            Self::Invalid(message) => formatter.write_str(message),
        }
    }
}

impl Error for VectorError {}

impl From<serde_json::Error> for VectorError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptFixture {
    pub schema_id: String,
    pub root_type: String,
    pub receipt_count: usize,
    pub receipts: Vec<ReceiptInput>,
    #[serde(default)]
    pub first_target: Option<FirstTarget>,
    pub source_scenario: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptInput {
    pub tx_type: u8,
    pub success: bool,
    pub gas_used: u64,
    pub contract_address: Option<String>,
    pub logs: Vec<LogInput>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogInput {
    pub address: String,
    pub topics: Vec<String>,
    pub data: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirstTarget {
    pub receipt_index: usize,
    pub log_index: usize,
    pub field: String,
}

pub(crate) fn validate_receipt_fixture(fixture: &ReceiptFixture) -> Result<(), VectorError> {
    require(fixture.schema_id == SCHEMA_ID, "unknown fixture schema")?;
    require(fixture.root_type == ROOT_TYPE, "unknown fixture root type")?;
    require(
        fixture.receipt_count == fixture.receipts.len(),
        "declared receipt count does not match fixture",
    )?;
    require(!fixture.source_scenario.trim().is_empty(), "fixture source scenario is empty")?;

    for receipt in &fixture.receipts {
        if let Some(address) = &receipt.contract_address {
            decode_fixed::<20>(address)?;
        }
        for log in &receipt.logs {
            decode_fixed::<20>(&log.address)?;
            require(log.topics.len() <= 4, "log has more than four topics")?;
            for topic in &log.topics {
                decode_fixed::<32>(topic)?;
            }
            decode_hex(&log.data)?;
        }
    }
    Ok(())
}

pub(crate) fn decode_hex(value: &str) -> Result<Vec<u8>, VectorError> {
    let digits = value
        .strip_prefix("0x")
        .ok_or_else(|| VectorError::Invalid("hex value is missing 0x prefix".into()))?;
    require(digits.len().is_multiple_of(2), "hex value has odd length")?;

    digits
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or_else(|| VectorError::Invalid("hex value contains invalid digits".into()))
        })
        .collect()
}

pub(crate) fn decode_fixed<const N: usize>(value: &str) -> Result<[u8; N], VectorError> {
    decode_hex(value)?.try_into().map_err(|value: Vec<u8>| {
        VectorError::Invalid(format!("expected {N} bytes, got {}", value.len()))
    })
}

fn require(condition: bool, message: &str) -> Result<(), VectorError> {
    condition.then_some(()).ok_or_else(|| VectorError::Invalid(message.into()))
}
