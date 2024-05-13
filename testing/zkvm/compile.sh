# CARGO_ENCODED_RUSTFLAGS


#         // Construct command from the toolchain-specific cargo
#         let mut cmd =
#             Command::new(cargo.map_or("cargo".to_string(), |c| String::from(c.to_str().unwrap())));
#         // Clear unwanted env vars
#         self.sanitize(&mut cmd, true);
#         cmd.current_dir(ROOT_DIR.get().unwrap());

#         // Set Rustc compiler path and flags
#         cmd.env(
#             "RUSTC",
#             rustc.map_or("rustc".to_string(), |c| String::from(c.to_str().unwrap())),
#         );
#         if let Some(rust_flags) = rust_flags {
#             cmd.env(
#                 "CARGO_ENCODED_RUSTFLAGS",
#                 format_flags("-C", &rust_flags).join("\x1f"),
#             );
#         }

#         // Set C compiler path and flags
#         if let Some(cc_compiler) = cc_compiler {
#             cmd.env("CC", cc_compiler);
#         }
#         if let Some(c_flags) = c_flags {
#             cmd.env(format!("CC_{}", self.target), c_flags.join(" "));
#         }

export PATH="/opt/riscv/bin:$PATH"
export RUSTUP_TOOLCHAIN="succinct"
export RUSTFLAGS="-C passes=loweratomic -C link-arg=-Ttext=0x00200800 -C panic=abort" 
export CARGO_BUILD_TARGET="riscv32im-succinct-zkvm-elf"
export CC="gcc"
export CC_riscv32im_succinct_zkvm_elf="/opt/riscv/bin/riscv32-unknown-elf-gcc -mstrict-align" 
cargo build --release --ignore-rust-version


# export PATH="$HOME/riscv/bin:$PATH"
# export RUSTUP_TOOLCHAIN="succinct"
# export RUSTFLAGS="-C passes=loweratomic -C link-arg=-Ttext=0x00200800 -C panic=abort" 
# export CARGO_BUILD_TARGET="riscv32im-succinct-zkvm-elf"
# export CC="gcc"
# export CC_riscv32im_succinct_zkvm_elf="$HOME/riscv/bin/riscv32-unknown-elf-gcc -mstrict-align" 
# cargo build --release --ignore-rust-version

# https://github.com/taikoxyz/raiko/blob/6a8421d5c17a7d8846c22f8f986d9584a6885bf1/script/install.sh#L4