## Installation

To use Reth, you must first install Geth. You can find instructions for installing Geth at the following link: [https://geth.ethereum.org/docs/install-and-build/installing-geth](https://geth.ethereum.org/docs/getting-started/installing-geth).

### Ubuntu
Building the source

* install rustup
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

* install Requirements
 - libclang-dev
 - pkg-config

```bash
apt install libclang-dev pkg-config
```
* Build reth
```bash
git clone https://github.com/paradigmxyz/reth
cd reth
cargo build --all
./target/debug/reth
```

### MacOS

To install and build reth on macOS using Homebrew, you can use the following steps

* Install rustup by running the following command in a terminal:
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

* Install the necessary requirements by running the following command:
```bash
brew install llvm pkg-config
```

* Clone the reth repository and navigate to the directory:
```bash
git clone https://github.com/paradigmxyz/reth
cd reth
```

* Build reth using cargo:
```bash
cargo build --all
```

* Run reth:
```bash
./target/debug/reth
```

* Alternatively, you can use the following one-liner to install and build reth:
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh && brew install llvm pkg-config && git clone https://github.com/paradigmxyz/reth && cd reth && cargo build --all && ./target/debug/reth
```
