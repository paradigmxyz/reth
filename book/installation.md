## Installation

To test Reth, you will need to have Geth and Lighthouse installed. 

#### Mac
```bash
brew install geth lighthouse
```

#### Ubuntu
1. First, update the package list:
```bash
sudo apt-get update
```


2. Then, install Geth:
```bash
sudo apt-get install geth
```


To install Lighthouse, you will need to follow the instructions on the Lighthouse GitHub page (https://github.com/sigp/lighthouse). This typically involves cloning the repository and building from source.

Here is the steps to install Lighthouse on Ubuntu:

1. Install the dependencies:
```bash
sudo apt-get install build-essential pkg-config libssl-dev libcurl4-openssl-dev libgmp-dev libboost-all-dev libsecp256k1-dev
```

2. Clone the repository:
```bash
git clone https://github.com/sigp/lighthouse.git
```

	
3. Build and install Lighthouse:
```bash
cd lighthouse
cargo build --release
sudo cargo install --path .
```


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



### Docker
-
- 