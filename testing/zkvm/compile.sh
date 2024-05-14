export PATH="/opt/riscv/bin:$PATH"
export RUSTUP_TOOLCHAIN="succinct"
export RUSTFLAGS="-C passes=loweratomic -C link-arg=-Ttext=0x00200800 -C panic=abort" 
export CARGO_BUILD_TARGET="riscv32im-succinct-zkvm-elf"
export CC="gcc"
export CC_riscv32im_succinct_zkvm_elf="/opt/riscv/bin/riscv32-unknown-elf-gcc -mstrict-align" 
cargo build --release --ignore-rust-version