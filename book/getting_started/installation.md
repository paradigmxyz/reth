## Installation

reth is currently unstable and does not have officially cut releases. To install reth, you must either install it from source or using Docker.

### From Source

To build from source, first install [Rust](https://rustup.rs). You also need some basic build tools for our C libraries:

- For Ubuntu: `apt-get install libclang-dev pkg-config`
- For Arch: `pacman -S base-devel`
- For macOS: `brew install llvm pkg-config`

Then clone the repository and build the binary:

```console
git clone https://github.com/paradigmxyz/reth
cd reth
cargo install --release --locked --path . --bin reth
```

The binary will now be in a platform specific folder, and should be accessible as `reth` via the command line.

### Using Docker

Clone the repository and build the image:

```console
git clone https://github.com/paradigmxyz/reth
docker build -t paradigmxyz/reth .
```
