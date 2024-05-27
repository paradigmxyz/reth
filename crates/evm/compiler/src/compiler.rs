use crate::{is_evm_version, EvmCompilerError, EvmCompilerResult, EvmVersions, SymbolBuffer};
use indexmap::{map::Entry, IndexMap};
use rayon::prelude::*;
use reth_fs_util as fs;
use reth_primitives::{keccak256, Bytes, B256};
use revm::primitives::SpecId;
use revm_jit::{debug_time, llvm, EvmCompiler, EvmLlvmBackend, Linker};
use serde::{Deserialize, Serialize};
use std::{
    fmt, io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU8, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

// TODO: Invalidate metadata if the compiler dependency version changes.

const BG_INTERVAL: Duration = Duration::from_secs(5);
const OBJ_NAME: &str = if cfg!(windows) { "object.obj" } else { "object.o" };

/// List of contracts to compile.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContractsConfig {
    /// The contracts to compile.
    pub contracts: Vec<CompilerContract>,
}

impl ContractsConfig {
    /// Creates a new configuration with the given contracts.
    pub fn new(contracts: Vec<CompilerContract>) -> Self {
        Self { contracts }
    }

    /// Loads the configuration from the given path.
    pub fn load(path: &Path) -> EvmCompilerResult<Self> {
        let mut this: Self = confy::load_path(path)?;
        let dir = path.parent().unwrap();
        for (i, contract) in this.contracts.iter_mut().enumerate() {
            contract.load_and_validate(i, dir)?;
        }
        Ok(this)
    }
}

/// EVM bytecode compiler contract configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompilerContract {
    /// The deployed bytecode.
    #[serde(default)]
    bytecode: Bytes,
    /// The path to the deployed bytecode. Ignored if `bytecode` is set.
    #[serde(default, skip_serializing)]
    bytecode_path: Option<PathBuf>,
    /// The EVM version to compile the contract with. Takes priority over `first_evm_version` and
    /// `last_evm_version`.
    #[serde(default)]
    evm_version: Option<SpecId>,
    /// The first EVM version to compile the contract with.
    #[serde(default = "EvmVersions::first")]
    first_evm_version: SpecId,
    /// The last EVM version to compile the contract with.
    #[serde(default = "EvmVersions::last")]
    last_evm_version: SpecId,
    /// Disables stack bound checks.
    ///
    /// **WARNING**: Disabling stack checks can lead to undefined behavior at runtime if the EVM
    /// stack overflows. This is generally impossible on most compiled contracts, but can still
    /// happen on unverified contracts, and should not be enabled unless you are certain that the
    /// bytecode *never* overflows the stack.
    #[serde(default)]
    unsafe_no_stack_bound_checks: bool,
}

impl CompilerContract {
    fn load_and_validate(&mut self, i: usize, config_dir: &Path) -> EvmCompilerResult<()> {
        if let Some(path) = &self.bytecode_path {
            if self.bytecode.is_empty() {
                let mut bytecode = fs::read(config_dir.join(path))?;
                if let Some(mut stripped) = bytecode.strip_prefix(b"0x") {
                    if let Ok(stripped_utf8) = std::str::from_utf8(stripped) {
                        stripped = stripped_utf8.trim().as_bytes();
                    }
                    bytecode = reth_primitives::hex::decode(stripped)?;
                }
                self.bytecode = bytecode.into();
            }
        }
        if self.bytecode.is_empty() {
            return Err(EvmCompilerError::EmptyBytecode(i));
        }

        if let Some(evm_version) = self.evm_version {
            if !is_evm_version(evm_version) {
                return Err(EvmCompilerError::InvalidEvmVersion(i, evm_version));
            }
        }

        if !is_evm_version(self.first_evm_version) || self.first_evm_version > EvmVersions::last() {
            return Err(EvmCompilerError::InvalidEvmVersion(i, self.first_evm_version));
        }
        if !is_evm_version(self.last_evm_version) || self.last_evm_version > EvmVersions::last() {
            return Err(EvmCompilerError::InvalidEvmVersion(i, self.last_evm_version));
        }

        if self.first_evm_version > self.last_evm_version {
            return Err(EvmCompilerError::InvalidEvmVersionRange(i));
        }

        Ok(())
    }
}

/// EVM bytecode parallel compiler.
pub struct EvmParCompiler {
    out_dir: PathBuf,
    metadata_path: PathBuf,
    metadata: Metadata,
    debug: bool,
    background_tasks: bool,
    force_link: bool,
    linker: Linker,
}

impl fmt::Debug for EvmParCompiler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EvmCompiler2")
            .field("out_dir", &self.out_dir)
            .field("metadata_path", &self.metadata_path)
            .field("metadata", &format_args!("..."))
            .field("debug", &self.debug)
            .field("background_tasks", &self.background_tasks)
            .field("linker", &self.linker)
            .finish()
    }
}

impl EvmParCompiler {
    /// Create a new EVM compiler that writes output to the given directory.
    pub fn new(out_dir: PathBuf) -> EvmCompilerResult<Self> {
        fs::create_dir_all(&out_dir)?;
        let evm_versions = EvmVersions::enabled();
        for evm_version in evm_versions {
            fs::create_dir_all(out_dir.join(format!("{evm_version:?}")))?;
        }
        let metadata_path = out_dir.join("meta.json");
        let mut metadata = if metadata_path.exists() {
            Metadata::load(&metadata_path)?
        } else {
            Metadata::new(evm_versions.len())
        };
        if metadata.spec_id_count != evm_versions.len() {
            // if metadata.spec_id_count > evm_versions.len() {
            //     panic!("invalid spec_id_count in metadata");
            // }
            metadata.resize_spec_ids(evm_versions.len());
        }
        Ok(Self {
            out_dir,
            metadata_path,
            metadata,
            debug: false,
            background_tasks: !cfg!(test),
            force_link: false,
            linker: Linker::new(),
        })
    }

    /// Sets the C compiler to use for linking.
    pub fn with_cc(mut self, cc: Option<PathBuf>) -> Self {
        self.linker.cc(cc);
        self
    }

    /// Sets extra C compiler arguments to use for linking.
    pub fn with_cflags(mut self, cflags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.linker.cflags(cflags);
        self
    }

    /// Sets whether to emit debug output.
    pub fn debug(&mut self, debug: bool) {
        self.debug = debug;
    }

    /// Runs the compiler until all contracts are compiled and linked.
    pub fn run_to_end(&mut self, contracts: &ContractsConfig) -> EvmCompilerResult<()> {
        self.add_contracts(&contracts.contracts);
        self.compile_and_link_all()?;
        self.save()?;
        Ok(())
    }

    /// Returns the hashes of all linked contracts.
    pub fn linked_contracts(&self) -> impl Iterator<Item = (SpecId, &B256)> + '_ {
        self.metadata.contracts_at(ContractState::Linked).map(|(v, h, _)| (v, h))
    }

    /// Adds the given contracts to the compiler. Won't overwrite existing contracts.
    pub fn add_contracts<'a>(&mut self, contracts: impl IntoIterator<Item = &'a CompilerContract>) {
        let contracts = contracts.into_iter();
        debug!("adding {:?} contracts", contracts.size_hint());
        for contract in contracts {
            let hash = keccak256(&contract.bytecode);
            let contract = self.metadata.contract_or_insert(hash, contract);
            for (_, state) in contract.states() {
                if state.get() == ContractState::None {
                    state.set(ContractState::ToCompile);
                }
            }
        }
    }

    /// Compiles the given contracts to object files.
    pub fn compile_and_link_all(&self) -> EvmCompilerResult<()> {
        debug_time!("compile and link all", || self.compile_and_link_all_inner())
    }

    fn compile_and_link_all_inner(&self) -> EvmCompilerResult<()> {
        let to_compile = debug_time!("get contracts", || self
            .metadata
            .contracts_at(ContractState::ToCompile)
            .collect::<Vec<_>>());
        if to_compile.is_empty() {
            debug!("no contracts to compile");
        } else {
            info!("starting to compile {} contracts", to_compile.len());
            let stopwatch = Instant::now();

            let total = to_compile.len();
            self.with_bg_tasks(total, |tx| {
                to_compile
                    .par_iter()
                    .map(|&(evm_version, hash, contract)| {
                        let start = Instant::now();
                        self.compile_one(evm_version, hash, &contract.contract)?;
                        let elapsed = start.elapsed();
                        let _ = tx.send(BgNotification::CompiledOne(evm_version, *hash, elapsed));
                        Ok(())
                    })
                    .collect::<EvmCompilerResult<()>>()
            })?;

            if !self.background_tasks {
                info!(elapsed=?stopwatch.elapsed(), "compiled {} contracts", to_compile.len());
            }
        }

        // This is a bit more complicated.
        // Since shared libraries cannot be extended, we have to re-link all of the objects if there
        // are any new ones.
        let mut new_objects = false;
        let to_link = debug_time!("get objects", || self
            .metadata
            .all_contracts()
            .filter(|(.., s)| {
                let s = s.get();
                if s == ContractState::Object {
                    new_objects = true;
                }
                matches!(s, ContractState::Object | ContractState::Linked)
            })
            .map(|(i, h, c, _)| (i, h, c))
            .collect::<Vec<_>>());
        if !self.force_link && (to_link.is_empty() || !new_objects) {
            debug!("no contracts to link");
        } else {
            let mut by_spec_id = std::iter::repeat_with(|| Vec::with_capacity(to_link.len()))
                .take(self.metadata.spec_id_count)
                .collect::<Vec<_>>();
            for &(evm_version, hash, contract) in &to_link {
                by_spec_id[evm_version as usize].push((hash, contract));
            }
            let n_dlls = by_spec_id.iter().filter(|c| !c.is_empty()).count();
            let n_to_link = to_link.len();

            info!("starting to link {n_dlls} shared libraries containing {n_to_link} contracts in total");
            let stopwatch = Instant::now();

            by_spec_id
                .par_iter()
                .enumerate()
                .filter(|(_, c)| !c.is_empty())
                .map(|(i, contracts)| -> io::Result<()> {
                    let spec_id = SpecId::try_from_u8(i as u8).unwrap();
                    let spec_id_s = format!("{spec_id:?}");
                    let spec_id_dir = self.out_dir.join(&spec_id_s);
                    let dll = self.out_dir.join(dll_filename(&spec_id_s));
                    let objects = contracts
                        .iter()
                        .map(|&(hash, _)| spec_id_dir.join(format!("{hash:x}/{OBJ_NAME}")));
                    self.linker.link(&dll, objects)?;
                    for (_, contract) in contracts {
                        contract.state(spec_id).set(ContractState::Linked);
                    }
                    Ok(())
                })
                .collect::<io::Result<()>>()?;

            info!(elapsed=?stopwatch.elapsed(), "linked {n_dlls} shared libraries");
        }

        Ok(())
    }

    fn compile_one(
        &self,
        evm_version: SpecId,
        hash: &B256,
        contract: &CompilerContract,
    ) -> EvmCompilerResult<()> {
        llvm::with_llvm_context(|cx| self.compile_one_with_context(evm_version, hash, contract, cx))
    }

    #[instrument(name = "compile", level = "debug", skip_all)]
    fn compile_one_with_context(
        &self,
        evm_version: SpecId,
        hash: &B256,
        contract: &CompilerContract,
        cx: &llvm::Context,
    ) -> EvmCompilerResult<()> {
        let compile_error = |e| EvmCompilerError::Compile(*hash, e);

        let contract_dir = self.out_dir.join(format!("{evm_version:?}/{hash:x}"));
        fs::create_dir_all(&contract_dir)?;

        let opt_level = revm_jit::OptimizationLevel::Aggressive;
        let backend = EvmLlvmBackend::new(cx, true, opt_level).map_err(compile_error)?;
        let mut compiler = EvmCompiler::new(backend);
        if self.debug {
            compiler.set_dump_to(Some(contract_dir.clone()));
        }
        unsafe { compiler.stack_bound_checks(contract.unsafe_no_stack_bound_checks) };

        trace!("compiling contract to object file");
        let name = SymbolBuffer::symbol(hash);
        let _id = compiler
            .translate(Some(&name), &contract.bytecode, evm_version)
            .map_err(compile_error)?;

        let object_file_path = contract_dir.join(OBJ_NAME);
        compiler.write_object_to_file(&object_file_path).map_err(compile_error)?;

        let state = self.metadata.state(hash, evm_version);
        debug_assert_eq!(state.get(), ContractState::ToCompile);
        state.set(ContractState::Object);

        Ok(())
    }

    fn with_bg_tasks(
        &self,
        total: usize,
        f: impl FnOnce(mpsc::Sender<BgNotification>) -> EvmCompilerResult<()>,
    ) -> EvmCompilerResult<()> {
        let (tx, rx) = mpsc::channel();

        if !self.background_tasks || total <= 1 {
            return f(tx);
        }

        thread::scope(|scope| -> EvmCompilerResult<()> {
            let builder = thread::Builder::new().name("evm-compiler-bg-tasks".into());
            let bg = builder.spawn_scoped(scope, move || -> EvmCompilerResult<()> {
                let mut n = 0;
                let max_n = total.ilog10() as usize + 1;

                let mut last_save = Instant::now();
                let mut save = || {
                    if last_save.elapsed() >= BG_INTERVAL {
                        let r = self.save();
                        last_save = Instant::now();
                        r
                    } else {
                        Ok(())
                    }
                };

                let mut durations = Vec::new();

                let start = Instant::now();
                loop {
                    match rx.recv_timeout(BG_INTERVAL) {
                        Ok(BgNotification::CompiledOne(evm_version, hash, elapsed)) => {
                            durations.push(elapsed);
                            n += 1;
                            info!(
                                "({n: >max_n$}/{total}) compiled contract {hash} \
                                 for EVM version {evm_version:?} in {elapsed:?}"
                            );
                            save()?;
                            if n == total {
                                break;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => save()?,
                    }
                }
                let elapsed = start.elapsed();

                let min = durations.iter().min().copied().unwrap_or_default();
                let max = durations.iter().max().copied().unwrap_or_default();
                let mean = durations.iter().sum::<Duration>() / durations.len() as u32;
                durations.sort_unstable();
                let median = if durations.len() % 2 == 0 {
                    (durations[durations.len() / 2 - 1] + durations[durations.len() / 2]) / 2
                } else {
                    durations[durations.len() / 2]
                };
                info!(
                    "compiled {n} contracts in {elapsed:.3?} \
                     (min: {min:.3?}, max: {max:.3?}, μ: {mean:.3?}, ~: {median:.3?})"
                );

                Ok(())
            })?;

            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(tx)));
            resume_panic(bg.join(), "panic in background thread")?;
            resume_panic(res, "panic during contract compilation")?;
            Ok(())
        })
    }

    fn save(&self) -> fs::Result<()> {
        debug_time!("save metadata", || self.metadata.save(&self.metadata_path))
    }
}

impl Drop for EvmParCompiler {
    fn drop(&mut self) {
        if let Err(err) = self.save() {
            error!(%err, "failed to save EVM compiler metadata");
        }
    }
}

enum BgNotification {
    CompiledOne(SpecId, B256, Duration),
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Metadata {
    /// Length of the states Vec below.
    spec_id_count: usize,
    /// hash -> (contract, evm_version -> state)
    contracts: IndexMap<B256, MetadataContract>,
}

impl Metadata {
    fn new(spec_id_count: usize) -> Self {
        Self { spec_id_count, contracts: IndexMap::new() }
    }

    fn load(path: &Path) -> fs::Result<Self> {
        debug!(?path, "loading metadata");
        fs::read_json_file(path)
    }

    fn save(&self, path: &Path) -> fs::Result<()> {
        debug!(?path, "saving metadata");
        fs::write_json_file(path, self)
    }

    fn resize_spec_ids(&mut self, new_count: usize) {
        debug!(from = self.spec_id_count, to = new_count, "updating spec_id_count");
        for contract in self.contracts.values_mut() {
            contract.states.resize_with(new_count, AtomicContractState::default);
        }
    }

    #[must_use]
    fn contract(&self, hash: &B256) -> &MetadataContract {
        &self.contracts[hash]
    }

    #[must_use]
    fn state(&self, hash: &B256, spec_id: SpecId) -> &AtomicContractState {
        self.contract(hash).state(spec_id)
    }

    #[must_use]
    #[instrument(level = "trace", skip(self, contract))]
    fn contract_or_insert(
        &mut self,
        hash: B256,
        contract: &CompilerContract,
    ) -> &mut MetadataContract {
        match self.contracts.entry(hash) {
            Entry::Occupied(e) => {
                trace!("updating");
                let c = e.into_mut();
                c.update(contract);
                c
            }
            Entry::Vacant(e) => {
                trace!("inserting");
                e.insert(MetadataContract::new(contract.clone(), self.spec_id_count))
            }
        }
    }

    fn contracts_at(
        &self,
        state: ContractState,
    ) -> impl Iterator<Item = (SpecId, &B256, &MetadataContract)> + '_ {
        self.all_contracts().filter(move |(.., s)| s.get() == state).map(|(i, h, c, _)| (i, h, c))
    }

    fn all_contracts(
        &self,
    ) -> impl Iterator<Item = (SpecId, &B256, &MetadataContract, &AtomicContractState)> {
        self.contracts.iter().flat_map(|(h, c)| c.states().map(move |(i, s)| (i, h, c, s)))
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct MetadataContract {
    contract: CompilerContract,
    /// `spec_id -> state`, state is `None` by default or if not an EVM version.
    states: Vec<AtomicContractState>,
}

impl MetadataContract {
    fn new(contract: CompilerContract, spec_id_count: usize) -> Self {
        Self {
            contract,
            states: std::iter::repeat_with(AtomicContractState::default)
                .take(spec_id_count)
                .collect(),
        }
    }

    fn state(&self, spec_id: SpecId) -> &AtomicContractState {
        assert!(is_evm_version(spec_id), "invalid spec_id");
        &self.states[spec_id as usize]
    }

    #[allow(dead_code)]
    fn state_mut(&mut self, spec_id: SpecId) -> &mut AtomicContractState {
        assert!(is_evm_version(spec_id), "invalid spec_id");
        &mut self.states[spec_id as usize]
    }

    fn states(&self) -> impl Iterator<Item = (SpecId, &AtomicContractState)> {
        let first = self.contract.evm_version.unwrap_or(self.contract.first_evm_version);
        let last = self.contract.evm_version.unwrap_or(self.contract.last_evm_version);
        EvmVersions::enabled()
            .iter_starting_at(first)
            .filter(move |spec_id| *spec_id <= last)
            .map(|spec_id| (spec_id, self.state(spec_id)))
    }

    fn update(&mut self, contract: &CompilerContract) {
        debug_assert_eq!(self.contract.bytecode, contract.bytecode);
        for (_, state) in self.states() {
            if state.get() == ContractState::None {
                state.set(ContractState::ToCompile);
            }
        }
        self.contract.unsafe_no_stack_bound_checks = contract.unsafe_no_stack_bound_checks;
    }
}

/// The state of a contract.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
enum ContractState {
    // NOTE: Order matters.
    /// Ignored.
    #[default]
    None,
    /// Needs to be compiled to an object.
    ToCompile,
    /// Object file is available, and needs to be linked together with other objects.
    Object,
    /// Object file is available, and is linked in a shared library.
    Linked,
}

impl ContractState {
    fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::None),
            1 => Some(Self::ToCompile),
            2 => Some(Self::Object),
            3 => Some(Self::Linked),
            _ => None,
        }
        .inspect(|s| debug_assert_eq!(*s as u8, value))
    }
}

/// Atomic version of [`ContractState`].
#[derive(Serialize)]
struct AtomicContractState(AtomicU8);

impl PartialEq for AtomicContractState {
    fn eq(&self, other: &Self) -> bool {
        self.get() == other.get()
    }
}

impl fmt::Debug for AtomicContractState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(f)
    }
}

impl<'de> Deserialize<'de> for AtomicContractState {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        u8::deserialize(deserializer).and_then(Self::try_from_de::<D>)
    }
}

impl Default for AtomicContractState {
    fn default() -> Self {
        Self::new(ContractState::None)
    }
}

impl AtomicContractState {
    fn new(state: ContractState) -> Self {
        Self(AtomicU8::new(state as u8))
    }

    fn try_from_de<'de, D: serde::Deserializer<'de>>(
        value: u8,
    ) -> std::result::Result<Self, D::Error> {
        if value <= ContractState::Linked as u8 {
            Ok(Self(AtomicU8::new(value)))
        } else {
            Err(serde::de::Error::custom("invalid EVM compiler contract state"))
        }
    }

    fn get(&self) -> ContractState {
        ContractState::from_u8(self.0.load(Ordering::Relaxed)).unwrap()
    }

    #[allow(dead_code)]
    fn get_mut(&mut self) -> ContractState {
        ContractState::from_u8(*self.0.get_mut()).unwrap()
    }

    fn set(&self, state: ContractState) {
        self.0.store(state as u8, Ordering::Relaxed)
    }

    #[allow(dead_code)]
    fn set_mut(&mut self, state: ContractState) {
        *self.0.get_mut() = state as u8;
    }
}

/// Returns the **file** name of a generated shared library.
pub(crate) fn dll_filename_spec_id(spec_id: SpecId) -> String {
    format!(
        "{prefix}reth_evm_compiler_{name:?}{suffix}",
        prefix = std::env::consts::DLL_PREFIX,
        name = crate::spec_id_to_evm_version(spec_id),
        suffix = std::env::consts::DLL_SUFFIX,
    )
}

/// Returns the **file** name of a generated shared library.
pub(crate) fn dll_filename(name: impl std::fmt::Display) -> String {
    format!(
        "{prefix}reth_evm_compiler_{name}{suffix}",
        prefix = std::env::consts::DLL_PREFIX,
        suffix = std::env::consts::DLL_SUFFIX,
    )
}

fn resume_panic<T>(res: Result<T, Box<dyn std::any::Any + 'static + Send>>, msg: &str) -> T {
    res.unwrap_or_else(|err| resume_panic_err(err, msg))
}

fn resume_panic_err(err: Box<dyn std::any::Any + 'static + Send>, msg: &str) -> ! {
    error!("{msg}");
    std::panic::resume_unwind(err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives::hex;
    use similar_asserts::assert_eq;

    const TESTDATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../testdata/compiler/");

    #[test]
    fn empty() {
        reth_tracing::init_test_tracing();
        let tmp_dir = tempfile::tempdir().unwrap();
        let root = tmp_dir.path();

        let contracts = ContractsConfig::default();
        let mut compiler = EvmParCompiler::new(root.into()).unwrap();
        compiler.run_to_end(&contracts).unwrap();
        let compiled = compiler.linked_contracts().collect::<Vec<_>>();
        assert!(compiled.is_empty());

        let meta_path = tmp_dir.path().join("meta.json");
        assert!(meta_path.exists());
        let meta = Metadata::load(&meta_path).unwrap();
        let expected_meta =
            Metadata { spec_id_count: EvmVersions::enabled().len(), contracts: IndexMap::new() };
        assert_eq!(meta, expected_meta);

        assert_fs(root, &default_fs());
    }

    #[test]
    fn simple() {
        reth_tracing::init_test_tracing();
        let tmp_dir = tempfile::tempdir().unwrap();
        let root = tmp_dir.path();

        let bytecode = &hex!("6001600201")[..];
        let hash = keccak256(bytecode);
        let contract = CompilerContract {
            bytecode: Bytes::from(bytecode),
            bytecode_path: None,
            evm_version: None,
            first_evm_version: SpecId::FRONTIER_THAWING,
            last_evm_version: SpecId::CANCUN,
            unsafe_no_stack_bound_checks: false,
        };
        let contracts = ContractsConfig::new(vec![contract.clone()]);
        let mut compiler = EvmParCompiler::new(root.into()).unwrap();
        compiler.run_to_end(&contracts).unwrap();
        let compiled = compiler.linked_contracts().collect::<Vec<_>>();
        assert_eq!(compiled, EvmVersions::enabled().iter().map(|v| (v, &hash)).collect::<Vec<_>>());

        let meta_path = root.join("meta.json");
        assert!(meta_path.exists());
        let meta = Metadata::load(&meta_path).unwrap();
        let spec_id_count = EvmVersions::enabled().len();
        let expected_meta = Metadata {
            spec_id_count,
            contracts: IndexMap::from([(
                hash,
                MetadataContract {
                    contract,
                    states: (0..spec_id_count)
                        .map(|i| {
                            let spec_id = SpecId::try_from_u8(i as u8).unwrap();
                            let expected_state = if is_evm_version(spec_id) {
                                ContractState::Linked
                            } else {
                                ContractState::None
                            };
                            AtomicContractState::new(expected_state)
                        })
                        .collect(),
                },
            )]),
        };
        assert_eq!(meta, expected_meta);

        let mut expected_fs = default_fs();
        for i in (0..expected_fs.len() - 1).rev() {
            let spec_id_dir = &expected_fs[i];
            let dll_name = dll_filename(spec_id_dir.file_name().unwrap().to_str().unwrap());
            let contract_dir = spec_id_dir.join(format!("{hash:x}"));
            expected_fs.insert(i + 1, spec_id_dir.with_file_name(dll_name));
            expected_fs.insert(i + 1, contract_dir.join(OBJ_NAME));
            expected_fs.insert(i + 1, contract_dir);
        }
        expected_fs.sort();
        assert_fs(root, &expected_fs);
    }

    #[test]
    #[ignore = "ran manually"]
    fn manual() {
        reth_tracing::init_test_tracing();
        let root = Path::new("/tmp/reth-evm-compiler-test/manual");

        let contracts_path = Path::new(TESTDATA).join("simple.toml");
        let contracts = ContractsConfig::load(&contracts_path).expect("failed to load contracts");
        let mut compiler = EvmParCompiler::new(root.into()).unwrap();
        let spec_id = SpecId::CANCUN;
        // for c in compiler.metadata.contracts.values_mut() {
        //     c.contract.starting_evm_version = spec_id;
        // }
        // for (_, _, c, _) in compiler.metadata.all_contracts() {
        //     c.state(SpecId::CANCUN).set(ContractState::None);
        // }
        // compiler.save().unwrap();
        compiler.background_tasks = true;
        compiler.force_link = true;
        compiler.run_to_end(&contracts).unwrap();

        let dll_path = root.join(dll_filename_spec_id(spec_id));
        let mut dll = unsafe { crate::EvmCompilerDll::open(&dll_path) }.unwrap();
        let function = dll.get_function(B256::ZERO).unwrap();
        assert!(function.is_none());
        let function = dll.get_function(B256::with_last_byte(1)).unwrap();
        assert!(function.is_none());

        let hashes = compiler.linked_contracts().filter(|(v, _)| *v == spec_id);
        for (_, hash) in hashes {
            let contract = compiler.metadata.contract(hash);
            let state = contract.state(spec_id);
            assert_eq!(state.get(), ContractState::Linked);
            let function = dll.get_function(*hash).unwrap().unwrap();

            let mut interpreter =
                revm::interpreter::Interpreter::new(Default::default(), u64::MAX, false);
            let mut host = revm::interpreter::DummyHost::default();
            // let t = Instant::now();
            let _r = unsafe { function.call_with_interpreter(&mut interpreter, &mut host) };
            // eprintln!("{:?} - {:?}", t.elapsed(), interpreter.instruction_result);
        }
    }

    fn assert_fs(root: &Path, expected: &[PathBuf]) {
        let actual = collect_fs(root);
        assert_eq!(actual[0], root);
        let rest: Vec<_> = actual[1..].iter().map(|p| p.strip_prefix(root).unwrap_or(p)).collect();
        assert_eq!(rest, expected);
    }

    fn collect_fs(root: &Path) -> Vec<PathBuf> {
        walkdir::WalkDir::new(root)
            .sort_by_file_name()
            .into_iter()
            .flat_map(Result::ok)
            .map(walkdir::DirEntry::into_path)
            .collect()
    }

    fn default_fs() -> Vec<PathBuf> {
        let mut fs: Vec<_> = EvmVersions::enabled()
            .iter()
            .map(|v| format!("{v:?}").into())
            .chain(std::iter::once(PathBuf::from("meta.json")))
            .collect();
        fs.sort();
        fs
    }
}
