## Installation

(still WIP, please contribute!):


### Ubuntu 
   Building the source

* install rustup
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
* install Requirements
   - libclang-dev 
   - pkg-config
    ```bash
   apt install libclang-dev pkg-config
* Build reth
    ```bash
   git clone https://github.com/paradigmxyz/reth
   cd reth
   cargo build --all
   ./target/debug/reth
   ```
### Docker
  -
  - 