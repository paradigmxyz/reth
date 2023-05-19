/// Expands to the short version information for reth.
///
/// - The latest version from Cargo.tom
/// - The short SHA of the latest commit.
///
/// Example: "1.2.3 (defa64b2)"
#[macro_export]
macro_rules! short_version {
    () => {
        concat!(env!("CARGO_PKG_VERSION"), " (", env!("VERGEN_GIT_SHA"), ")")
    };
}

/// Expands to the long version information for reth.
///
/// Expands to the short version information for reth.
///
/// - The latest version from Cargo.tom
/// - The long SHA of the latest commit.
/// - The build datetime
/// - The build features
///
/// Example: "1.2.3 (defa64b2)"
#[macro_export]
macro_rules! long_version {
    () => {
        concat!(
            "Version: ",
            env!("CARGO_PKG_VERSION"),
            "\n",
            "Commit SHA: ",
            env!("VERGEN_GIT_SHA"),
            "\n",
            "Build Timestamp: ",
            env!("VERGEN_BUILD_TIMESTAMP"),
            "\n",
            "Build Features: ",
            env!("VERGEN_CARGO_FEATURES")
        )
    };
}
