pub mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

fn main() {
    let version = built_info::PKG_VERSION;
    let name = built_info::PKG_NAME;
    let sha = built_info::GIT_COMMIT_HASH_SHORT;
    let os = std::env::consts::OS;
    let rustc = built_info::RUSTC_VERSION;

    println!("{}/v{}/{}-{}/{}", name, version, os, sha.unwrap(), rustc);

    if let Err(err) = reth::cli::run() {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
