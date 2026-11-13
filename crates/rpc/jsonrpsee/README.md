# jsonrpsee

Vendored copy of [jsonrpsee](https://github.com/paritytech/jsonrpsee), maintained as part of reth.

Imported from upstream `master` at commit `243343e55f` (v0.26.1 plus #1642 and #1662). The crate
names are kept, so `use jsonrpsee::...` works unchanged. The code is licensed under the MIT
license in [`LICENSE`](./LICENSE); the upstream copyright headers are kept in each source file.

The copy only carries the server side that reth uses. The client crates are gone, so reth's clients
use alloy (`alloy-rpc-client`, `alloy-provider`), and `#[rpc(server)]` generates only the server
trait. There is also no HTTP host-filter or proxy middleware and no low-level `TowerService` API.
