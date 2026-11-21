//! In-process deterministic differential workload with the version-1 native ABI.

use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    any::Any,
    ffi::c_void,
    fmt,
    panic::{catch_unwind, AssertUnwindSafe},
    path::Path,
    ptr, slice,
    sync::Mutex,
    time::Instant,
};

#[path = "../../../crates/trie/sparse/examples/sparse_trie_differential.rs"]
#[expect(dead_code, reason = "The shared example also has a standalone main function.")]
mod core;

mod input;

struct Workload {
    poisoned: bool,
    pending: Option<Value>,
}

// The loader registers writable module-lifetime allocations before workers are forked.
// No Rust reference into them survives a call to instrumented code.
struct CounterSpan {
    start: *mut u8,
    len: usize,
}

// SAFETY: These pointers refer to static SanCov storage, retained until process exit by the
// native ABI. The registry mutex serializes reset/run/read across every workload handle.
unsafe impl Send for CounterSpan {}

#[derive(Default)]
struct CounterRegistry {
    spans: Vec<CounterSpan>,
    invalid: bool,
}

struct NativeCoverage {
    counters: Vec<u8>,
    id: String,
}

const MAX_COVERAGE_SIZE: usize = 67_108_864;
const MAX_COUNTER_SPANS: usize = 4096;
static NATIVE_COUNTERS: Mutex<CounterRegistry> =
    Mutex::new(CounterRegistry { spans: Vec::new(), invalid: false });

const MAX_REQUEST_BYTES: usize = 65_536;
const SEED_COUNT: u64 = 72;
const EXTRA_SEEDS: [u64; 8] = [64, 127, 128, 255, 256, 65_535, u64::MAX - 1, u64::MAX];

type Reply = unsafe extern "C" fn(*mut c_void, *const u8, usize);

#[derive(Debug)]
enum Error {
    Invalid(&'static str),
    Json(serde_json::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::Json(error) => write!(f, "invalid workload JSON: {error}"),
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    args: Empty,
    search: Search,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Search {
    finalize_every: u64,
}

#[derive(Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "kebab-case", deny_unknown_fields)]
enum Request {
    SeedInputs {
        args: Empty,
        index: Option<u64>,
    },
    FreshInput {
        args: Empty,
        seed: u64,
    },
    MutationOperators {
        args: Empty,
    },
    MutationSites {
        args: Empty,
        input: String,
        operator: String,
    },
    MutateInput {
        args: Empty,
        input: String,
        seed: u64,
        operator: String,
        path: Vec<String>,
        donor: Option<String>,
        donor_path: Option<Vec<String>>,
    },
    Execute {
        input: String,
        seed: u64,
        out: String,
    },
    Finish {
        admitted: bool,
        n: u64,
    },
}

impl Workload {
    fn call(&mut self, request: Request) -> Result<Value, Error> {
        if self.pending.is_some() && !matches!(request, Request::Finish { .. }) {
            return Err(Error::Invalid("finish is required before another call"));
        }
        match request {
            Request::SeedInputs { args: Empty {}, index } => match index {
                None => Ok(json!({"count": SEED_COUNT})),
                Some(index) if index < SEED_COUNT => {
                    let seed = if index < 64 { index } else { EXTRA_SEEDS[(index - 64) as usize] };
                    Ok(json!({"input": input::generated(seed)?}))
                }
                Some(_) => Err(Error::Invalid("seed index is out of range")),
            },
            Request::FreshInput { args: Empty {}, seed } => {
                Ok(json!({"input": input::generated(mix(seed))?}))
            }
            Request::MutationOperators { args: Empty {} } => Ok(input::operators()),
            Request::MutationSites { args: Empty {}, input, operator } => {
                let input = input::parse(&input)?;
                Ok(serde_json::to_value(input::sites(&input, &operator)?)?)
            }
            Request::MutateInput {
                args: Empty {},
                input,
                seed,
                operator,
                path,
                donor,
                donor_path,
            } => {
                let input = input::parse(&input)?;
                let donor = match (donor, donor_path) {
                    (Some(other), Some(path)) => Some((input::parse(&other)?, path)),
                    (None, None) => None,
                    (None, Some(path)) if path.is_empty() => None,
                    _ => {
                        return Err(Error::Invalid("donor and donor_path must be supplied together"))
                    }
                };
                Ok(Value::String(input::mutate(input, &operator, &path, seed, donor)?))
            }
            Request::Execute { input, seed: _execution_seed, out } => {
                let input = input::parse(&input)?;
                if !Path::new(&out).is_absolute() {
                    return Err(Error::Invalid("out must be an absolute path"));
                }
                // The complete input determines execution; orchestration randomness is ignored.
                let started = Instant::now();
                let (result, native) = run_measured(&input)?;
                let duration = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
                let result = execution_result(result, duration, native)?;
                Ok(self.retain_execution(result))
            }
            Request::Finish { admitted: _admitted, n: _number } => {
                self.pending.take().ok_or(Error::Invalid("no execution is pending"))
            }
        }
    }

    fn retain_execution(&mut self, mut result: Value) -> Value {
        if result["coverage_size"] != 0 {
            self.pending = Some(result.clone());
            result["pending"] = json!(true);
        }
        result
    }
}

fn mix(seed: u64) -> u64 {
    let mut value = seed.wrapping_add(0x9e3779b97f4a7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    value ^ (value >> 31)
}

fn execution_result(
    result: Result<String, core::DifferentialFailure>,
    duration: u64,
    native: NativeCoverage,
) -> Result<Value, Error> {
    let coverage = native
        .counters
        .iter()
        .enumerate()
        .filter(|(_, count)| **count != 0)
        .map(|(slot, count)| json!([slot, count]))
        .collect::<Vec<_>>();
    let coverage_size = native.counters.len();
    let report = match result {
        Ok(report) => serde_json::from_str::<Value>(&report)?,
        Err(failure) => {
            if failure.signature.is_empty() {
                return Err(Error::Invalid("core returned an empty objective signature"));
            }
            // Native edges remain available even when the oracle exits before its report.
            return Ok(json!({
                "coverage": coverage, "coverage_size": coverage_size, "coverage_id": native.id,
                "semantic_features": [],
                "verdict": {"status": "difference", "details": failure.details},
                "objective_signatures": [failure.signature], "duration_ns": duration,
                "pending": false, "finalized": true
            }));
        }
    };
    if coverage_size == 0 {
        return Err(Error::Invalid("native coverage is absent; build with prepare.sh"));
    }
    let mut features = report
        .get("features")
        .and_then(Value::as_array)
        .ok_or(Error::Invalid("core report has no feature array"))?
        .iter()
        .map(|feature| {
            feature
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .ok_or(Error::Invalid("core report has an invalid feature"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let operations = report
        .get("operations")
        .and_then(Value::as_object)
        .ok_or(Error::Invalid("core report has no operation counts"))?;
    if operations.keys().any(|name| !core::COVERAGE_OPERATIONS.contains(&name.as_str())) {
        return Err(Error::Invalid("core reported an unregistered coverage operation"));
    }
    for operation in core::COVERAGE_OPERATIONS {
        if let Some(count) = operations.get(*operation) {
            let count = count.as_u64().ok_or(Error::Invalid("core report has an invalid count"))?;
            if count > 0 {
                features.push(format!("operation/{operation}/log2/{}", count.ilog2()));
            }
        }
    }
    features.sort_unstable();
    features.dedup();
    Ok(json!({
        "coverage": coverage, "coverage_size": coverage_size,
        "coverage_id": native.id,
        "semantic_features": features,
        "verdict": {"status": "passed", "report": report},
        "objective_signatures": [], "duration_ns": duration,
        "pending": false, "finalized": true
    }))
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        format!("workload panicked; handle poisoned: {message}")
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        format!("workload panicked; handle poisoned: {message}")
    } else {
        "workload panicked; handle poisoned: non-string payload".to_owned()
    }
}

/// Creates a handle after worker fork; success does not invoke `reply`.
///
/// # Safety
/// `config` must reference `len` readable bytes (or be null for zero length). The callback must
/// remain valid for this call, must not reenter this library, and must not unwind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rift_workload_create_v1(
    config: *const u8,
    len: usize,
    reply: Reply,
    context: *mut c_void,
) -> *mut c_void {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: The caller supplies the documented readable request buffer.
        let bytes = unsafe { request_bytes(config, len)? };
        let Config { args: Empty {}, search } = serde_json::from_slice(bytes)?;
        if search.finalize_every != 1 {
            return Err(Error::Invalid("finalize_every must be 1"));
        }
        Ok(Box::new(Workload { poisoned: false, pending: None }))
    }));
    match result {
        Ok(Ok(handle)) => Box::into_raw(handle).cast(),
        Ok(Err(error)) => {
            // SAFETY: The callback borrows these bytes only until it returns.
            unsafe { send(reply, context, error.to_string().as_bytes()) };
            ptr::null_mut()
        }
        Err(payload) => {
            // SAFETY: The callback borrows these bytes only until it returns.
            unsafe { send(reply, context, panic_message(payload.as_ref()).as_bytes()) };
            ptr::null_mut()
        }
    }
}

/// Calls one method, invoking `reply` exactly once with JSON or a UTF-8 diagnostic.
///
/// # Safety
/// The handle must be live and exclusive to this thread. `request` must reference `len` readable
/// bytes (or be null for zero length). The callback must not reenter this library or unwind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rift_workload_call_v1(
    handle: *mut c_void,
    request: *const u8,
    len: usize,
    reply: Reply,
    context: *mut c_void,
) -> i32 {
    if handle.is_null() {
        // SAFETY: The caller supplies a valid synchronous callback.
        unsafe { send(reply, context, b"null workload handle") };
        return 1;
    }
    // SAFETY: The caller provides a live, exclusive handle returned by create.
    let workload = unsafe { &mut *handle.cast::<Workload>() };
    let (status, bytes) = guarded_call(workload, |workload| {
        // SAFETY: The caller supplies the documented readable request buffer.
        let bytes = unsafe { request_bytes(request, len)? };
        let request = serde_json::from_slice(bytes)?;
        serde_json::to_vec(&workload.call(request)?).map_err(Error::from)
    });
    // SAFETY: The callback borrows the reply only until it returns.
    unsafe { send(reply, context, &bytes) };
    status
}

fn guarded_call(
    workload: &mut Workload,
    call: impl FnOnce(&mut Workload) -> Result<Vec<u8>, Error>,
) -> (i32, Vec<u8>) {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if workload.poisoned {
            return Err(Error::Invalid("workload handle is poisoned"));
        }
        call(workload)
    }));
    match result {
        Ok(Ok(bytes)) => (0, bytes),
        Ok(Err(error)) => (1, error.to_string().into_bytes()),
        Err(payload) => {
            workload.poisoned = true;
            workload.pending = None;
            (1, panic_message(payload.as_ref()).into_bytes())
        }
    }
}

/// Releases a handle once, after its final call.
///
/// # Safety
/// The pointer must be null or a live handle from create, not concurrently used or previously
/// freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rift_workload_destroy_v1(handle: *mut c_void) {
    if !handle.is_null() {
        // Destructors are contained so no Rust unwind crosses the ABI boundary.
        let _caught = catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: The caller transfers unique ownership of the live handle.
            unsafe { drop(Box::from_raw(handle.cast::<Workload>())) };
        }));
    }
}

unsafe fn request_bytes<'a>(bytes: *const u8, len: usize) -> Result<&'a [u8], Error> {
    if len > MAX_REQUEST_BYTES {
        return Err(Error::Invalid("request exceeds 65536 bytes"));
    }
    if len == 0 {
        return Ok(&[]);
    }
    if bytes.is_null() {
        return Err(Error::Invalid("null request buffer"));
    }
    // SAFETY: The caller guarantees a readable buffer; null and length are checked above.
    Ok(unsafe { slice::from_raw_parts(bytes, len) })
}

unsafe fn send(reply: Reply, context: *mut c_void, bytes: &[u8]) {
    // SAFETY: The callback is synchronous and non-unwinding by the ABI contract.
    unsafe { reply(context, bytes.as_ptr(), bytes.len()) };
}

/// Registers one compiler-generated inline 8-bit counter allocation at library load.
///
/// # Safety
/// `start..end` must be a single writable allocation that remains mapped until process exit.
/// Only compiler constructors may call this function, before any workload execution. The host
/// must not execute this library's instrumented code outside its sequential workload calls.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __sanitizer_cov_8bit_counters_init(start: *mut u8, end: *mut u8) {
    // No unwind or user callbacks are possible here, including after mutex poisoning.
    let mut registry = match NATIVE_COUNTERS.lock() {
        Ok(registry) => registry,
        Err(poisoned) => poisoned.into_inner(),
    };
    registry.register(start, end);
}

impl CounterRegistry {
    fn register(&mut self, start: *mut u8, end: *mut u8) {
        if start == end {
            return;
        }
        let Some(len) = end.addr().checked_sub(start.addr()) else {
            self.invalid = true;
            return;
        };
        if start.is_null() || len > MAX_COVERAGE_SIZE {
            self.invalid = true;
            return;
        }
        if self.spans.iter().any(|span| span.start == start && span.len == len) {
            return;
        }
        if self.spans.len() == MAX_COUNTER_SPANS ||
            self.spans.iter().map(|span| span.len).sum::<usize>() > MAX_COVERAGE_SIZE - len
        {
            self.invalid = true;
            return;
        }
        self.spans.push(CounterSpan { start, len });
    }

    // SAFETY requirements are inherited from the constructor registration contract. Holding
    // the registry lock excludes execution through other handles. No references alias writes
    // performed by the target: raw pointers are used only before and after core::run.
    unsafe fn clear(&self) {
        for span in &self.spans {
            // SAFETY: Each registered allocation remains writable for its recorded length.
            unsafe { ptr::write_bytes(span.start, 0, span.len) };
        }
    }

    unsafe fn snapshot(&self) -> NativeCoverage {
        let mut counters = Vec::new();
        let mut layout = 0xcbf29ce484222325_u64;
        for span in &self.spans {
            for byte in span.len.to_le_bytes() {
                layout = (layout ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            }
            for offset in 0..span.len {
                // SAFETY: Instrumented execution has ended and the lock prevents another run.
                counters.push(unsafe { span.start.add(offset).read_volatile() });
            }
        }
        // The wrapper hashes the actual instrumented rlib, so a different build cannot silently
        // reuse a layout just because its number of counters happens to be unchanged.
        let build = option_env!("SPARSE_TRIE_NATIVE_COVERAGE_ID").unwrap_or("uninstrumented");
        let id = if counters.is_empty() {
            String::new()
        } else {
            format!("sparse-trie-sancov-v1-{build}-{layout:016x}")
        };
        NativeCoverage { counters, id }
    }
}

fn run_measured(
    input: &core::Input,
) -> Result<(Result<String, core::DifferentialFailure>, NativeCoverage), Error> {
    let registry = NATIVE_COUNTERS
        .lock()
        .map_err(|_| Error::Invalid("native coverage registry is poisoned"))?;
    if registry.invalid {
        return Err(Error::Invalid("native coverage registration is invalid"));
    }
    if registry.spans.is_empty() {
        return Err(Error::Invalid("native coverage is absent; build with prepare.sh"));
    }
    // SAFETY: Loader registration guarantees static allocations. The mutex spans the complete
    // reset/run/read transaction. The adapter and core driver are deliberately uninstrumented.
    unsafe { registry.clear() };
    let result = core::run_input(input);
    // SAFETY: The core has returned and no target code can concurrently update the counters.
    let native = unsafe { registry.snapshot() };
    Ok((result, native))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_null_donor_and_empty_path_are_accepted() {
        let input = input::generated(0).unwrap();
        let path =
            input::sites(&input::parse(&input).unwrap(), "key.update").unwrap().remove(0).path;
        let request = json!({"method":"mutate-input","params":{
            "args":{},"input":input,"seed":99,"operator":"key.update","path":path,
            "donor":null,"donor_path":[]
        }});
        let mut workload = Workload { poisoned: false, pending: None };
        let result = workload.call(serde_json::from_value(request).unwrap()).unwrap();
        assert_ne!(result.as_str().unwrap(), input);
    }

    #[test]
    fn differences_are_objectives_not_errors() {
        let result = execution_result(
            Err(core::DifferentialFailure {
                signature: "root/mismatch".to_owned(),
                details: "expected != actual".to_owned(),
            }),
            12,
            native(&[]),
        )
        .unwrap();
        assert_eq!(result["objective_signatures"], json!(["root/mismatch"]));
        assert_eq!(result["verdict"]["status"], "difference");
        assert_eq!(result["finalized"], true);
    }

    #[test]
    fn pending_requires_finish() {
        let mut workload = Workload { poisoned: false, pending: Some(json!({"result": 1})) };
        assert_eq!(
            workload
                .call(Request::SeedInputs { args: Empty {}, index: None })
                .unwrap_err()
                .to_string(),
            "finish is required before another call"
        );
        assert_eq!(
            workload.call(Request::Finish { admitted: false, n: 1 }).unwrap(),
            json!({"result":1})
        );
        assert_eq!(
            workload.call(Request::Finish { admitted: false, n: 2 }).unwrap_err().to_string(),
            "no execution is pending"
        );
    }

    #[test]
    fn unexpected_panic_poisoning_is_permanent() {
        let mut workload = Workload { poisoned: false, pending: Some(json!({})) };
        let (status, bytes) = guarded_call(&mut workload, |_| panic!("test fault"));
        assert_eq!(status, 1);
        assert_eq!(bytes, b"workload panicked; handle poisoned: test fault");
        assert!(workload.poisoned);
        assert!(workload.pending.is_none());
        let (status, bytes) = guarded_call(&mut workload, |_| unreachable!("poison guard"));
        assert_eq!(status, 1);
        assert_eq!(bytes, b"workload handle is poisoned");
    }

    #[test]
    fn request_pointer_and_size_validation() {
        // SAFETY: These invalid pointers are rejected before accessing memory.
        unsafe {
            assert_eq!(
                request_bytes(ptr::null(), 1).unwrap_err().to_string(),
                "null request buffer"
            );
            assert_eq!(
                request_bytes(ptr::null(), MAX_REQUEST_BYTES + 1).unwrap_err().to_string(),
                "request exceeds 65536 bytes"
            );
            assert_eq!(request_bytes(ptr::null(), 0).unwrap(), b"");
        }
    }

    #[test]
    fn native_coverage_and_semantic_features_are_independent() {
        let name = core::COVERAGE_OPERATIONS[0];
        let report = |count| {
            Ok(json!({"features": ["scenario"], "operations": {(name): count}}).to_string())
        };
        let one = execution_result(report(1), 0, native(&[0, 1, 255])).unwrap();
        let many = execution_result(report(999), 0, native(&[0, 1, 255])).unwrap();
        assert_eq!(one["coverage"], json!([[1, 1], [2, 255]]));
        assert_eq!(many["coverage"], json!([[1, 1], [2, 255]]));
        assert_eq!(one["coverage_id"], many["coverage_id"]);
        assert_eq!(one["coverage_size"], 3);
        assert_ne!(one["semantic_features"], many["semantic_features"]);
        let empty = execution_result(
            Ok(json!({"features": [], "operations": {}}).to_string()),
            0,
            native(&[0, 0, 0]),
        )
        .unwrap();
        assert_eq!(one["coverage_id"], empty["coverage_id"]);
        assert_eq!(empty["coverage"], json!([]));
    }

    #[test]
    fn early_objective_is_final_not_pending() {
        let mut workload = Workload { poisoned: false, pending: None };
        let objective = execution_result(
            Err(core::DifferentialFailure {
                signature: "root/mismatch".to_owned(),
                details: "different".to_owned(),
            }),
            0,
            native(&[]),
        )
        .unwrap();
        assert_eq!(workload.retain_execution(objective.clone()), objective);
        assert_eq!(objective["pending"], false);
        assert!(workload.pending.is_none());
        assert_eq!(
            workload.call(Request::Finish { admitted: false, n: 1 }).unwrap_err().to_string(),
            "no execution is pending"
        );
    }

    fn native(counters: &[u8]) -> NativeCoverage {
        NativeCoverage {
            counters: counters.to_vec(),
            id: if counters.is_empty() { String::new() } else { "test-layout".to_owned() },
        }
    }

    #[test]
    fn absent_native_coverage_is_an_error_not_semantic_fallback() {
        let error = execution_result(
            Ok(json!({"features": [], "operations": {}}).to_string()),
            0,
            native(&[]),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "native coverage is absent; build with prepare.sh");
    }

    #[test]
    fn native_spans_reset_and_snapshot_without_self_observation() {
        let mut first = [1_u8, 2, 255];
        let mut second = [4_u8, 5];
        let mut registry = CounterRegistry::default();
        let first_start = first.as_mut_ptr();
        let second_start = second.as_mut_ptr();
        registry.register(first_start, first_start.wrapping_add(first.len()));
        registry.register(first_start, first_start.wrapping_add(first.len()));
        registry.register(second_start, second_start.wrapping_add(second.len()));
        registry.register(ptr::null_mut(), ptr::null_mut());
        assert_eq!(registry.spans.len(), 2);
        // SAFETY: Both arrays remain live, and no target or other thread accesses them.
        let before = unsafe { registry.snapshot() };
        assert_eq!(before.counters, [1, 2, 255, 4, 5]);
        // SAFETY: Both arrays are exclusively owned and remain writable.
        unsafe { registry.clear() };
        // SAFETY: Read and write through the original raw pointers without retagging arrays.
        assert_eq!(unsafe { registry.snapshot() }.counters, [0, 0, 0, 0, 0]);
        // SAFETY: This pointer still uniquely addresses the live first allocation.
        unsafe { first_start.add(1).write(7) };
        // SAFETY: No references to the arrays are live while snapshot reads their pointers.
        let after = unsafe { registry.snapshot() };
        assert_eq!(after.counters, [0, 7, 0, 0, 0]);
        assert_eq!(before.id, after.id);
        // SAFETY: Snapshot does not mutate either allocation.
        assert_eq!(after.counters, unsafe { registry.snapshot() }.counters);
    }

    #[test]
    fn invalid_native_span_is_rejected_without_dereferencing() {
        let mut registry = CounterRegistry::default();
        registry.register(ptr::null_mut(), ptr::without_provenance_mut(1));
        assert!(registry.invalid);
        assert!(registry.spans.is_empty());
    }

    #[test]
    fn objective_keeps_native_edges_and_requires_finish() {
        let mut workload = Workload { poisoned: false, pending: None };
        let result = execution_result(
            Err(core::DifferentialFailure {
                signature: "root/mismatch".to_owned(),
                details: "different".to_owned(),
            }),
            0,
            native(&[2]),
        )
        .unwrap();
        assert_eq!(result["coverage"], json!([[0, 2]]));
        assert_eq!(workload.retain_execution(result.clone())["pending"], true);
        assert_eq!(workload.call(Request::Finish { admitted: false, n: 1 }).unwrap(), result);
    }
}
