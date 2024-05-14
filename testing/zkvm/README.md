To run testing on a zkVM, you must run on a Linux machine.

First run `bash install.sh` to download the `riscv-gcc-prebuilt` toolchain. Note that this toolchain is only prebuilt for Linux machines, which is why you're required to use Linux.

Then, run `bash compile.sh` which will compile with this directory's `Cargo.toml` to the SP1 zkVM. If compilation succeeds, then this means we can use all of those crates within any program written for SP1.