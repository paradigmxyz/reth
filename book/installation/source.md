# Build from Source

You can build Reth on Linux, macOS, Windows, and Windows WSL2.

> **Note**
>
> Reth does **not** work on Windows WSL1.

## Dependencies

First, **install Rust** using [rustup](https://rustup.rs/)： 

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

The rustup installer provides an easy way to update the Rust compiler, and works on all platforms.

> **Tips**
>
> - During installation, when prompted, enter `1` for the default installation.
> - After Rust installation completes, try running `cargo version` . If it cannot
>   be found, run `source $HOME/.cargo/env`. After that, running `cargo version` should return the version, for example `cargo 1.68.2`.
> - It's generally advisable to append `source $HOME/.cargo/env` to `~/.bashrc`.

With Rust installed, follow the instructions below to install dependencies relevant to your
operating system:

- **Ubuntu**: `apt-get install libclang-dev pkg-config build-essential`
- **macOS**: `brew install llvm pkg-config`
- **Windows**: `choco install llvm` or `winget install LLVM.LLVM`

These are needed to build bindings for Reth's database.

## Build Reth

With Rust and the dependencies installed, you're ready to build Reth. First, clone the repository:

```bash
git clone https://github.com/paradigmxyz/reth
cd reth
```

Then, install Reth into your `PATH` directly via:

```bash
cargo install --locked --path bin/reth --bin reth
```

The binary will now be accessible as `reth` via the command line, and exist under your default `.cargo/bin` folder.

Alternatively, you can build yourself with:

```bash
cargo build --release
```

This will place the reth binary under `./target/release/reth`, and you can copy it to your directory of preference after that.

Compilation may take around 10 minutes. Installation was successful if `reth --help` displays the [command-line documentation](../cli/cli.md).

If you run into any issues, please check the [Troubleshooting](#troubleshooting) section, or reach out to us on [Telegram](https://t.me/paradigm_reth).

## Update Reth

You can update Reth to a specific version by running the commands below.

The `reth` directory will be the location you cloned reth to during the installation process.

`${VERSION}` will be the version you wish to build in the format `vX.X.X`.

```bash
cd reth
git fetch
git checkout ${VERSION}
cargo build --release
```

## Optimizations

**Profiles**

You can customise the compiler settings used to compile Reth via
[Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html).

Reth includes several profiles which can be selected via the Cargo flag `--profile`.

* `release`: default for source builds, enables most optimisations while not taking too long to
  compile.
* `maxperf`: default for binary releases, enables aggressive optimisations including full LTO.
  Although compiling with this profile improves some benchmarks by around 20% compared to `release`,
  it imposes a _significant_ cost at compile time and is only recommended if you have a fast CPU.

**Rust compiler flags**

You can also use `RUSTFLAGS="-C target-cpu=native"` to enable CPU-specific optimisations. In order to get
the highest performance out of your build:

```bash
RUSTFLAGS="-C target-cpu=native" cargo build --profile maxperf
```

**Features**

Finally, some optional features are present that may improve performance, but may not very portable,
and as such might not compile on your particular system. These are currently:
- `jemalloc`: replaces the default system memory allocator with [`jemalloc`](https://jemalloc.net/); this feature is unstable on Windows
- `asm-keccak`: replaces the default, pure-Rust implementation of Keccak256 with one implemented in assembly; see [the `keccak-asm` crate](https://github.com/DaniPopes/keccak-asm) for more details and supported targets
- `min-LEVEL-logs`, where `LEVEL` is one of `error`, `warn`, `info`, `debug`, `trace`: disables compilation of logs of lower level than the given one; this in general isn't that significant, and is not recommended due to the loss of debugging that the logs would provide

You can activate features by passing them to the `--features` or `-F` Cargo flag;
multiple features can be activated with a space- or comma-separated list to the flag:

```bash
RUSTFLAGS="-C target-cpu=native" cargo build --profile maxperf --features jemalloc,asm-keccak
```

## Troubleshooting

### Command is not found

Reth will be installed to `CARGO_HOME` or `$HOME/.cargo`. This directory needs to be on your `PATH` before you can run `$ reth`.

See ["Configuring the `PATH` environment variable"](https://www.rust-lang.org/tools/install) for more information.

### Compilation error

Make sure you are running the latest version of Rust. If you have installed Rust using rustup, simply run `rustup update`.

If you can't install the latest version of Rust you can instead compile using the Minimum Supported
Rust Version (MSRV) which is listed under the `rust-version` key in Reth's
[Cargo.toml](https://github.com/paradigmxyz/reth/blob/main/Cargo.toml).

If compilation fails with `(signal: 9, SIGKILL: kill)`, this could mean your machine ran out of
memory during compilation. If you are on Docker, consider increasing the memory of the container, or use a [pre-built
binary](../installation/binaries.md).

If compilation fails in either the `keccak-asm` or `sha3-asm` crates, it is likely that your current
system configuration is not supported. See the [`keccak-asm` target table](https://github.com/DaniPopes/keccak-asm?tab=readme-ov-file#support) for supported targets.

If compilation fails with `error: linking with cc failed: exit code: 1`, try running `cargo clean`.

_(Thanks to Sigma Prime for this section from [their Lighthouse book](https://lighthouse-book.sigmaprime.io/installation.html)!)_

### Bus error (WSL2)

In WSL 2 on Windows, the default virtual disk size is set to 1TB. 

You must increase the allocated disk size for your WSL2 instance before syncing reth.

You can follow the instructions here: [how to expand the size of your WSL2 virtual hard disk.](https://learn.microsoft.com/en-us/windows/wsl/disk-space#how-to-expand-the-size-of-your-wsl-2-virtual-hard-disk)
