//! Bedrock assertion macros for reth DST workloads.
//!
//! `always!` records a condition that must always hold; `sometimes!` records
//! one that must hold at least once. False observations are written without
//! panicking. Reporting errors panic with the assertion name.

use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use bedrock_assertions::Assertion;
use serde::Serialize;

#[doc(hidden)]
pub use bedrock_assertions as __bedrock;
#[doc(hidden)]
pub use serde_json as __serde_json;

static OUTPUT: OnceLock<io::Result<Option<Mutex<File>>>> = OnceLock::new();

/// Appends one assertion to `BEDROCK_ASSERTIONS_PATH` when it is set.
/// The output path is read once per process. Details are evaluated by the
/// caller but serialized only when the output is enabled.
#[doc(hidden)]
pub fn __report(assertion: Assertion, details: impl Serialize) {
    let result = (|| {
        let output = OUTPUT.get_or_init(|| {
            std::env::var_os("BEDROCK_ASSERTIONS_PATH")
                .map(|path| OpenOptions::new().create(true).append(true).open(path))
                .transpose()
                .map(|file| file.map(Mutex::new))
        });
        let Some(output) =
            output.as_ref().map_err(|error| io::Error::new(error.kind(), error.to_string()))?
        else {
            return Ok(());
        };

        let mut record = serde_json::to_value(&assertion).map_err(io::Error::other)?;
        let details = serde_json::to_value(details).map_err(io::Error::other)?;
        let kind = match assertion {
            Assertion::Always(_) => "Always",
            Assertion::Sometimes(_) => "Sometimes",
        };
        let data = record[kind].as_object_mut().expect("Bedrock assertion has an object payload");
        data.insert(
            "timestamp_unix_nano".into(),
            (SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u64)
                .into(),
        );
        if !details.is_null() {
            data.insert("details".into(), details);
        }
        let mut line = serde_json::to_vec(&record).map_err(io::Error::other)?;
        line.push(b'\n');
        if line.len() > 16 * 1024 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "DST record exceeds 16 KiB"));
        }
        let mut file = output.lock().map_err(|_| io::Error::other("DST writer poisoned"))?;
        // One append syscall keeps records from different processes from interleaving.
        if file.write(&line)? != line.len() {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "partial DST write"));
        }
        Ok::<_, io::Error>(())
    })();
    if let Err(error) = result {
        panic!("reth-dst could not report assertion {:?}: {error}", assertion.data().message);
    }
}

/// Records a safety assertion. The name must be a nonempty constant string.
#[macro_export]
macro_rules! always {
    ($condition:expr, $name:expr $(, $details:expr)? $(,)?) => {{
        $crate::__assert!($condition, $name, always $(, $details)?);
    }};
}

/// Records an assertion that must be true at least once.
/// The name must be a nonempty constant string.
#[macro_export]
macro_rules! sometimes {
    ($condition:expr, $name:expr $(, $details:expr)? $(,)?) => {{
        $crate::__assert!($condition, $name, sometimes $(, $details)?);
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __assert {
    ($condition:expr, $name:expr, $kind:ident $(,)?) => {{
        $crate::__assert!($condition, $name, $kind, $crate::__serde_json::Value::Null);
    }};
    ($condition:expr, $name:expr, $kind:ident, $details:expr $(,)?) => {{
        const NAME: &str = $name;
        const _: () = assert!(!NAME.is_empty(), "assertion name must not be empty");
        let condition: bool = $condition;
        let details = $details;
        $crate::__report(
            $crate::__bedrock::Assertion::$kind(
                $crate::__bedrock::Condition::Bool(condition),
                NAME,
                $crate::__bedrock::Location::new(file!(), line!(), column!()),
            ),
            details,
        );
    }};
}
