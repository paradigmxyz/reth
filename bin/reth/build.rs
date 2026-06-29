#![allow(missing_docs)]

fn main() {
    if std::env::var_os("CARGO_FEATURE_JIT").is_some() {
        evm2_jit_build::emit();
    }
}
