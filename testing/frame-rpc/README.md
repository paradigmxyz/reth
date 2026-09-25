# Frame RPC devnet checks

Run from this directory with Docker and Docker Compose:

```sh
docker compose up --build --abort-on-container-exit --exit-code-from tests
```

The node builds from the current checkout with JIT disabled. The first build compiles Reth and downloads Rust dependencies. Subsequent builds reuse Cargo caches.

The disposable chain enables Bogota at genesis, funds a public test key, and predeploys storage and revert fixtures. RPC is accessible only inside the Compose network. The chain data lives in a temporary filesystem.

The runner checks omitted limits, explicit zero, independent signing and raw submission, inclusion, cross-frame storage, rollback, fee accounting, and invalid signatures. Transactions and receipts must validate against execution-apis #907 at revision `94b90a4855233838b75ff54fc363b54f700f653e`.

Intentional reverts use explicit limits because filling omitted gas searches for successful execution. The reverting frame must fail without preventing the following frame from executing. The RPC receipt reports transaction status `0x0` and frame statuses `[0x1, 0x0, 0x1]`.

Each run needs fresh chain state. Remove the containers before rerunning:

```sh
docker compose down
```
