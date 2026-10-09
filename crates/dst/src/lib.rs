//! Property assertions for reth workloads running under Bedrock.
//!
//! Use [`always!`] for safety properties and [`sometimes!`] for reachability
//! properties. Each macro site appears in the compiled [`catalog`], even if
//! never reached. Call [`init`] at startup to publish the catalog when no macro
//! may execute.

use std::{
    error::Error,
    fmt,
    fs::{File, OpenOptions},
    io::{self, Write},
    path::Path,
    sync::{Arc, Mutex, OnceLock},
};

use bedrock_assertions::{Assertion, Condition, Location};
use serde::Serialize;

pub use serde_json::Value;

#[doc(hidden)]
pub use inventory as __inventory;

static WRITER: OnceLock<Result<Option<PropertyWriter>, ReportingError>> = OnceLock::new();

/// Initializes property reporting and publishes every linked property site.
///
/// Set `BEDROCK_ASSERTIONS_PATH` to a JSONL file in an existing directory to
/// enable reporting. Otherwise initialization succeeds with reporting disabled.
/// Enabled reporting creates the file if needed and appends records without
/// overwriting existing contents. Disabled reporting creates no file and does
/// not serialize argument values.
/// The first call permanently fixes the configuration and result for the
/// process, including disabled reporting or a failure; subsequent calls do not
/// retry initialization. Macros initialize reporting automatically if necessary.
///
/// Returns an error if reporting cannot be initialized or a catalog record
/// (including its newline) exceeds 16 KiB.
pub fn init() -> Result<(), ReportingError> {
    writer().map(|_| ())
}

/// A property's aggregation rule.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PropertyKind {
    /// Every observation must be true and the site must be reached.
    Always,
    /// At least one observation must be true.
    Sometimes,
}

/// An immutable compiled assertion site, obtained from [`catalog::properties`].
#[derive(Clone, Copy, Debug)]
pub struct Property {
    id: &'static str,
    kind: PropertyKind,
    file: &'static str,
    line: u32,
    column: u32,
}

impl Property {
    /// Returns the nonempty, constant property name used as its identity.
    pub const fn id(&self) -> &'static str {
        self.id
    }

    /// Returns the property's aggregation rule.
    pub const fn kind(&self) -> PropertyKind {
        self.kind
    }

    /// Returns the source file containing the macro call.
    pub const fn file(&self) -> &'static str {
        self.file
    }

    /// Returns the one-based source line of the macro call.
    pub const fn line(&self) -> u32 {
        self.line
    }

    /// Returns the one-based source column of the macro call.
    pub const fn column(&self) -> u32 {
        self.column
    }

    fn record(&self, condition: bool, details: Value, hit: bool) -> io::Result<Vec<u8>> {
        let location = Location::new(self.file, self.line, self.column);
        let assertion = match self.kind {
            PropertyKind::Always => {
                Assertion::always(Condition::Bool(condition), self.id, location)
            }
            PropertyKind::Sometimes => {
                Assertion::sometimes(Condition::Bool(condition), self.id, location)
            }
        };
        let data = assertion.data();
        let record = RecordData {
            condition: data.condition,
            result: data.result,
            message: self.id,
            location: &data.location,
            property_id: self.id,
            hit: (!hit).then_some(false),
            details,
        };
        let record = match self.kind {
            PropertyKind::Always => Record::Always(record),
            PropertyKind::Sometimes => Record::Sometimes(record),
        };
        let mut line = serde_json::to_vec(&record).map_err(io::Error::other)?;
        line.push(b'\n');
        if line.len() > 16 * 1024 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "DST record exceeds 16 KiB"));
        }
        Ok(line)
    }
}

/// A failure to initialize or export property reporting.
///
/// Reporting failures are fatal to property macros, which panic with the
/// property's name. Explicit initialization and catalog export return this
/// error to the caller instead.
#[derive(Clone, Debug)]
pub struct ReportingError {
    source: Arc<dyn Error + Send + Sync>,
}

impl ReportingError {
    fn new(source: impl Error + Send + Sync + 'static) -> Self {
        Self { source: Arc::new(source) }
    }
}

impl fmt::Display for ReportingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "could not report property assertions: {}", self.source)
    }
}

impl Error for ReportingError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}

#[derive(Debug)]
struct PropertyWriter {
    output: Mutex<File>,
}

impl PropertyWriter {
    fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let writer =
            Self { output: Mutex::new(OpenOptions::new().create(true).append(true).open(path)?) };
        for property in catalog::properties() {
            writer.write(&property.record(true, Value::Null, false)?)?;
        }
        Ok(writer)
    }

    fn write(&self, line: &[u8]) -> io::Result<()> {
        let mut output = self.output.lock().map_err(|_| io::Error::other("DST writer poisoned"))?;
        // One append syscall keeps records from different processes from interleaving.
        if output.write(line)? != line.len() {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "partial DST write"));
        }
        Ok(())
    }
}

fn writer() -> Result<Option<&'static PropertyWriter>, ReportingError> {
    WRITER
        .get_or_init(|| match std::env::var_os("BEDROCK_ASSERTIONS_PATH") {
            Some(path) => PropertyWriter::open(path).map(Some).map_err(ReportingError::new),
            None => Ok(None),
        })
        .as_ref()
        .map(|writer| writer.as_ref())
        .map_err(Clone::clone)
}

// Keep Bedrock's payload fields, but omit its wall-clock timestamp so records
// remain deterministic across checkpoint replay and catalog exports.
#[derive(Serialize)]
struct RecordData<'a> {
    condition: Condition,
    result: bool,
    message: &'static str,
    location: &'a Location,
    property_id: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    hit: Option<bool>,
    #[serde(skip_serializing_if = "Value::is_null")]
    details: Value,
}

#[derive(Serialize)]
enum Record<'a> {
    Always(RecordData<'a>),
    Sometimes(RecordData<'a>),
}

#[doc(hidden)]
pub fn __observe(property: &Property, condition: bool, details: impl Serialize) {
    let result = (|| {
        if let Some(writer) = writer()? {
            let details = serde_json::to_value(details).map_err(ReportingError::new)?;
            let line = property.record(condition, details, true).map_err(ReportingError::new)?;
            writer.write(&line).map_err(ReportingError::new)?;
        }
        Ok::<_, ReportingError>(())
    })();
    if let Err(error) = result {
        panic!("reth-dst could not report property {:?}: {error}", property.id);
    }
}

/// Compiled property sites, including sites that never execute.
pub mod catalog {
    use std::io::Write;

    use serde_json::Value;

    use crate::{Property, ReportingError};

    inventory::collect!(Property);

    /// Returns all linked sites ordered by ID, kind, file, line, and column.
    /// Multiple call sites may share an ID; each site is returned separately.
    pub fn properties() -> Vec<Property> {
        let mut properties: Vec<_> = inventory::iter::<Property>.into_iter().copied().collect();
        properties.sort_by_key(|p| (p.id, p.kind, p.file, p.line, p.column));
        properties
    }

    /// Exports declarations without initializing reporting or evaluating conditions.
    ///
    /// Writes Bedrock `Always` / `Sometimes` JSONL records with `hit: false` and
    /// `property_id` fields and no timestamp. Returns an error if writing fails
    /// or a record (including its newline) exceeds 16 KiB. Earlier declarations
    /// may already have been written when an error occurs.
    pub fn write(mut output: impl Write) -> Result<(), ReportingError> {
        for property in properties() {
            let line = property.record(true, Value::Null, false).map_err(ReportingError::new)?;
            output.write_all(&line).map_err(ReportingError::new)?;
        }
        Ok(())
    }
}

/// Records a safety property: every observation must be true and the site reached.
///
/// `always!(condition, name[, details])` returns `()` and records false
/// observations without panicking. `name` must be a nonempty constant string.
/// The boolean condition and optional [`Serialize`] details evaluate exactly
/// once, including with reporting disabled. Details serialize as a JSON value;
/// null details are omitted. With reporting disabled, details are evaluated but
/// not serialized. Reporting is configured by [`init`].
///
/// # Panics
///
/// Panics with the property name on a reporting failure, including an
/// initialization failure or a record (including its newline) exceeding 16 KiB.
#[macro_export]
macro_rules! always {
    ($condition:expr, $name:expr $(, $details:expr)? $(,)?) => {{
        $crate::__assert_property!($crate::PropertyKind::Always, $condition, $name $(, $details)?);
    }};
}

/// Records a reachability property: at least one observation must be true.
///
/// `sometimes!(condition, name[, details])` has the same argument, evaluation,
/// reporting, and panic contracts as [`always!`].
#[macro_export]
macro_rules! sometimes {
    ($condition:expr, $name:expr $(, $details:expr)? $(,)?) => {{
        $crate::__assert_property!($crate::PropertyKind::Sometimes, $condition, $name $(, $details)?);
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __assert_property {
    ($kind:expr, $condition:expr, $name:expr $(,)?) => {{
        $crate::__assert_property!($kind, $condition, $name, $crate::Value::Null);
    }};
    ($kind:expr, $condition:expr, $name:expr, $details:expr $(,)?) => {{
        const PROPERTY: $crate::Property =
            $crate::__property($kind, $name, file!(), line!(), column!());
        $crate::__inventory::submit! { PROPERTY }
        let condition: bool = $condition;
        let details = $details;
        $crate::__observe(&PROPERTY, condition, details);
    }};
}

#[doc(hidden)]
pub const fn __property(
    kind: PropertyKind,
    id: &'static str,
    file: &'static str,
    line: u32,
    column: u32,
) -> Property {
    assert!(!id.is_empty(), "property name must not be empty");
    Property { id, kind, file, line, column }
}
