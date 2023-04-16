// The file `built.rs` was placed there by cargo and `build.rs`
mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

fn main() {
    let version = built_info::PKG_VERSION;
    let name = built_info::PKG_NAME;
    let sha = built_info::GIT_COMMIT_HASH_SHORT;
    let os = std::env::consts::OS;
    let rustc = built_info::RUSTC_VERSION;

    println!("{name}/v{version}-{sha}/{os}/{rustc} \n", name, version, sha, os, rustc);

    if let Err(err) = reth::cli::run() {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
