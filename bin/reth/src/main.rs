// We use jemalloc for performance reasons
#[cfg(feature = "jemalloc")]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;
use eyre::ErrReport;
use std::{
    env,
    panic::{self, catch_unwind},
    sync::OnceLock,
};
/// Exit status code used for successful run of program.
pub const EXIT_SUCCESS: i32 = 0;

/// Exit status code used for panick failures.
pub const EXIT_FAILURE: i32 = 1;

type Panicked = dyn Fn(&panic::PanicInfo<'_>) + Sync + Send + 'static;
static DEFAULT_HOOK: OnceLock<Box<Panicked>> = OnceLock::new();

/// Appends custom panic hook to default message that will print on panics.
pub fn install_panic_hook() {
    // If the user has not explicitly overridden "RUST_BACKTRACE", then produce
    // full backtraces. When a panic happens, we want to gather
    // as much information as possible to present in the issue opened
    // by the user. Users can opt in to less-verbose backtraces
    // by manually setting "RUST_BACKTRACE" (e.g. `RUST_BACKTRACE=1`)
    if std::env::var("RUST_BACKTRACE").is_err() {
        std::env::set_var("RUST_BACKTRACE", "full");
    }

    let default_hook = DEFAULT_HOOK.get_or_init(panic::take_hook);

    panic::set_hook(Box::new(move |info| {
        // Invoke the default handler, which prints the actual panic message and optionally a
        // backtrace
        (*default_hook)(info);

        // Separate the output with an empty line
        eprintln!();

        // Print the custom panic message
        report_custom_panic();
    }));
}

/// Prints custom panic message
///
///
/// When `install_panic_hook` is called, this function will be called as the panic
/// hook.
pub fn report_custom_panic() {
    let url: &str = "https://github.com/paradigmxyz/reth/issues/new?assignees=&\
  labels=C-bug%2CS-needs-triage&projects=&template=bug.yml";
    eprintln!("Reth Panicked, Please report this at {}", url);

    let reth_version = option_env!("CARGO_PKG_VERSION").unwrap_or("unknown version");

    eprintln!("Reth Version: {}", reth_version);
    eprintln!("OS: {} {}", env::consts::OS, env::consts::ARCH);
    eprintln!("Cmd: \"{}\"", std::env::args().collect::<Vec<_>>().join(" "));
}

/// Runs a closure and catches unwinds triggered by fatal errors.
/// This function catches the panic into a `Result` instead.
pub fn catch_fatal_errors<F: FnOnce() -> R, R>(f: F) -> Result<R, ErrReport> {
    catch_unwind(panic::AssertUnwindSafe(f)).map_err(|value| {
        eyre::eyre!(value
            .downcast_ref::<&'static str>()
            .copied()
            .unwrap_or("unknown panic payload"))

        // TODO: maybe conditionally unwind depending on type of panic
        // if condition {report} else {panic::resume_unwind(value)}
    })
}

/// Variant of `catch_fatal_errors` for the `eyre::Result` return type
/// that also computes the exit code.
pub fn catch_with_exit_code(f: impl FnOnce() -> eyre::Result<()>) -> i32 {
    let result = catch_fatal_errors(f).and_then(|result| result);
    match result {
        Ok(()) => EXIT_SUCCESS,
        Err(_) => EXIT_FAILURE,
    }
}

fn main() {
    install_panic_hook();
    std::process::exit(catch_with_exit_code(|| reth::cli::run()));
}

#[cfg(test)]
mod tests {
    use crate::{catch_with_exit_code, install_panic_hook};

    #[test]
    fn test_panic() {
        install_panic_hook();
        let exit_code = catch_with_exit_code(|| panic!("test panic"));
        assert_eq!(exit_code, 1);
    }
    #[test]
    fn test_ok() {
        install_panic_hook();
        let exit_code = catch_with_exit_code(|| Ok(()));
        assert!(exit_code == 0);
    }
}
