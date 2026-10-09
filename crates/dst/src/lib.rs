//! Property assertions for reth workloads running under Bedrock.
//!
//! `always!` requires every observation to be true and the site to be reached.
//! `sometimes!` requires at least one true observation. Both return `()`,
//! record false observations without panicking, and panic on reporting errors.
//! Set `BEDROCK_ASSERTIONS_PATH` to append JSONL records. Without it, the
//! macros evaluate their arguments but do not serialize or write records.
//!
//! Each macro site is registered in a compiled catalog, even if never reached.
//! Call [`init`] at startup to publish that catalog when no macro may execute.

use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::Path,
    sync::{Mutex, OnceLock},
};

use bedrock_assertions::{Assertion, Condition, Location};
use serde::Serialize;
pub use serde_json::Value;

#[doc(hidden)]
pub use inventory as __inventory;

static SDK: OnceLock<io::Result<Option<Sdk>>> = OnceLock::new();

/// Initializes the shared writer and publishes every linked property site.
/// The output path is read once per process.
pub fn init() -> io::Result<Option<&'static Sdk>> {
    match SDK.get_or_init(|| match std::env::var_os("BEDROCK_ASSERTIONS_PATH") {
        Some(path) => Sdk::open(path).map(Some),
        None => Ok(None),
    }) {
        Ok(sdk) => Ok(sdk.as_ref()),
        Err(error) => Err(io::Error::new(error.kind(), error.to_string())),
    }
}

/// A property's aggregation rule.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Kind {
    /// Every observation must be true.
    Always,
    /// At least one observation must be true.
    Sometimes,
}

/// A compiled assertion site.
#[derive(Clone, Debug)]
pub struct Property {
    /// Stable property identity.
    pub id: String,
    /// Aggregation rule.
    pub kind: Kind,
    /// Macro call site.
    pub location: Location,
}

impl Property {
    fn record(&self, condition: bool, details: Value, hit: bool) -> io::Result<Value> {
        let assertion = match self.kind {
            Kind::Always => {
                Assertion::always(Condition::Bool(condition), &self.id, self.location.clone())
            }
            Kind::Sometimes => {
                Assertion::sometimes(Condition::Bool(condition), &self.id, self.location.clone())
            }
        };
        let mut record = serde_json::to_value(assertion).map_err(io::Error::other)?;
        let kind = match self.kind {
            Kind::Always => "Always",
            Kind::Sometimes => "Sometimes",
        };
        let data = record[kind].as_object_mut().expect("Bedrock assertion has an object payload");
        // The upstream type adds a wall-clock timestamp; property records must
        // remain deterministic across checkpoint replay and catalog exports.
        data.remove("timestamp_unix_nano");
        if !hit {
            data.insert("hit".into(), false.into());
        }
        data.insert("property_id".into(), self.id.clone().into());
        if !details.is_null() {
            data.insert("details".into(), details);
        }
        Ok(record)
    }
}

/// A file sink shared by all property macros in a process.
#[derive(Debug)]
pub struct Sdk {
    output: Mutex<File>,
}

impl Sdk {
    /// Opens an explicit JSONL path and writes the complete compiled catalog.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let sdk =
            Self { output: Mutex::new(OpenOptions::new().create(true).append(true).open(path)?) };
        for property in catalog::properties() {
            sdk.write(&property.record(true, Value::Null, false)?)?;
        }
        Ok(sdk)
    }

    /// Records one observation without treating a false condition as an error.
    pub fn observe(&self, property: &Property, condition: bool, details: Value) -> io::Result<()> {
        self.write(&property.record(condition, details, true)?)
    }

    fn write(&self, record: &Value) -> io::Result<()> {
        let mut line = serde_json::to_vec(record).map_err(io::Error::other)?;
        line.push(b'\n');
        if line.len() > 16 * 1024 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "DST record exceeds 16 KiB"));
        }
        let mut output = self.output.lock().map_err(|_| io::Error::other("DST writer poisoned"))?;
        // One append syscall keeps records from different processes from interleaving.
        if output.write(&line)? != line.len() {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "partial DST write"));
        }
        Ok(())
    }
}

#[doc(hidden)]
pub fn __observe(property: &Property, condition: bool, details: impl Serialize) {
    let result = (|| {
        if let Some(sdk) = init()? {
            sdk.observe(
                property,
                condition,
                serde_json::to_value(details).map_err(io::Error::other)?,
            )?;
        }
        Ok::<_, io::Error>(())
    })();
    if let Err(error) = result {
        panic!("reth-dst could not report property {:?}: {error}", property.id);
    }
}

/// Compiled property sites, including sites that never execute.
pub mod catalog {
    use std::io::{self, Write};

    use bedrock_assertions::Location;
    use serde_json::Value;

    use crate::{Kind, Property};

    /// Linker registration for a property macro site.
    #[doc(hidden)]
    #[derive(Debug)]
    pub struct Declaration {
        /// Stable ID.
        pub id: &'static str,
        /// Aggregation rule.
        pub kind: Kind,
        /// Source file.
        pub file: &'static str,
        /// Source line.
        pub line: u32,
        /// Source column.
        pub column: u32,
    }

    inventory::collect!(Declaration);

    /// Returns all linked sites in deterministic order.
    pub fn properties() -> Vec<Property> {
        let mut entries: Vec<_> = inventory::iter::<Declaration>.into_iter().collect();
        entries.sort_by_key(|p| (p.id, p.kind, p.file, p.line, p.column));
        entries
            .into_iter()
            .map(|p| Property {
                id: p.id.into(),
                kind: p.kind,
                location: Location::new(p.file, p.line, p.column),
            })
            .collect()
    }

    /// Exports declarations without opening a sink or evaluating conditions.
    pub fn write(mut output: impl Write) -> io::Result<()> {
        for property in properties() {
            let mut line = serde_json::to_vec(&property.record(true, Value::Null, false)?)
                .map_err(io::Error::other)?;
            line.push(b'\n');
            if line.len() > 16 * 1024 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "DST declaration exceeds 16 KiB",
                ));
            }
            output.write_all(&line)?;
        }
        Ok(())
    }
}

/// Records a safety property. The name must be a nonempty constant string.
#[macro_export]
macro_rules! always {
    ($condition:expr, $name:expr $(, $details:expr)? $(,)?) => {{
        $crate::__assert_property!($crate::Kind::Always, $condition, $name $(, $details)?);
    }};
}

/// Records a reachability property. The name must be a nonempty constant string.
#[macro_export]
macro_rules! sometimes {
    ($condition:expr, $name:expr $(, $details:expr)? $(,)?) => {{
        $crate::__assert_property!($crate::Kind::Sometimes, $condition, $name $(, $details)?);
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __assert_property {
    ($kind:expr, $condition:expr, $name:expr $(,)?) => {{
        $crate::__assert_property!($kind, $condition, $name, $crate::Value::Null);
    }};
    ($kind:expr, $condition:expr, $name:expr, $details:expr $(,)?) => {{
        const ID: &str = $name;
        const _: () = assert!(!ID.is_empty(), "property name must not be empty");
        $crate::__inventory::submit! {
            $crate::catalog::Declaration {
                id: ID, kind: $kind, file: file!(), line: line!(), column: column!()
            }
        }
        let property = $crate::Property {
            id: ID.into(),
            kind: $kind,
            location: $crate::__location(file!(), line!(), column!()),
        };
        let condition: bool = $condition;
        let details = $details;
        $crate::__observe(&property, condition, details);
    }};
}

#[doc(hidden)]
pub fn __location(file: &str, line: u32, column: u32) -> Location {
    Location::new(file, line, column)
}
