#![allow(missing_docs)]

fn main() {
    #[cfg(feature = "compiler")]
    revm_jit_build::emit();
}
