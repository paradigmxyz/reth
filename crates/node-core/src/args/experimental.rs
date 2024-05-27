use clap::Parser;
use std::path::PathBuf;

/// Experimental arguments.
#[derive(Clone, Debug, Default, Parser)]
#[command(next_help_heading = "Experimental")]
pub struct ExperimentalArgs {
    /// The EVM bytecode compiler arguments.
    #[command(flatten)]
    pub compiler: CompilerArgs,
}

impl ExperimentalArgs {
    /// Returns `true` if any experimental feature is enabled.
    pub fn any_is_enabled(&self) -> bool {
        self.compiler.compiler
    }
}

/// Experimental EVM bytecode compiler arguments.
#[derive(Clone, Debug, Parser)]
#[command(next_help_heading = "Compiler")]
pub struct CompilerArgs {
    /// Enable the experimental EVM bytecode compiler.
    ///
    /// This will compile all bytecodes defined in `contracts-file`/`contracts.toml` ahead of time,
    /// and then load them dynamically when the EVM is invoked with a bytecode that matches one of
    /// the compiled bytecodes.
    #[arg(long = "experimental.compiler")]
    pub compiler: bool,
    /// Block on finishing the compiler before starting the node.
    #[arg(long = "experimental.compiler.block-on-compiler")]
    pub block_on_compiler: bool,
    /// Path to a file that contains all the contracts to compile.
    ///
    /// Defaults to `<datadir>/compiler/contracts.toml`.
    #[arg(long = "experimental.compiler.contracts-file")]
    pub contracts_file: Option<PathBuf>,
    /// Directory in which intermediate artifacts, metadata, and results are stored.
    ///
    /// Defaults to `<datadir>/compiler/artifacts/`.
    #[arg(long = "experimental.compiler.out-dir")]
    pub out_dir: Option<PathBuf>,
    /// Which C compiler to use for linking.
    #[arg(long = "experimental.compiler.cc")]
    pub cc: Option<PathBuf>,
    /// Additional arguments to pass to the C compiler when linking.
    #[arg(long = "experimental.compiler.cflags")]
    pub cflags: Vec<String>,
}

#[allow(clippy::derivable_impls)]
impl Default for CompilerArgs {
    fn default() -> Self {
        Self {
            compiler: false,
            block_on_compiler: false,
            contracts_file: None,
            out_dir: None,
            cc: None,
            cflags: vec![],
        }
    }
}
