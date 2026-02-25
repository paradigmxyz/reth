fn main() {
    #[cfg(target_arch = "x86_64")]
    {
        cc::Build::new()
            .file("xkcp/KeccakP-1600-times4-AVX2.c")
            .file("xkcp/keccak256_4x.c")
            .include("xkcp")
            .flag("-mavx2")
            .flag("-O3")
            .flag("-std=c99")
            .warnings(false)
            .compile("keccak_times4");
    }
}
