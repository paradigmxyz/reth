#![allow(missing_docs)]

fn main() {
    #[cfg(feature = "compiler")]
    revmc_build::emit();
}
