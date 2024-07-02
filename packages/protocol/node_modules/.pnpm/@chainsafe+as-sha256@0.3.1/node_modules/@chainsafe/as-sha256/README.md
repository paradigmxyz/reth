# as-sha256

![ES Version](https://img.shields.io/badge/ES-2015-yellow)
![Node Version](https://img.shields.io/badge/node-12.x-green)

AssemblyScript implementation of SHA256.

## Usage

`yarn add @chainsafe/as-sha256`

```typescript
import {digest, digest64, SHA256} from "@chainsafe/as-sha256";

let hash: Uint8Array;

// create a new sha256 context
const sha256 = new SHA256();
// with init(), update(data), and final()
hash = sha256.init().update(Buffer.from("Hello world")).final();

// or use a one-pass interface
hash = digest(Buffer.from("Hello world"));

// or use a faster one-pass interface for hashing (only) 64 bytes
hash = digest64(Buffer.from("abcdefghabcdefghabcdefghabcdefghabcdefghabcdefghabcdefghabcdefgh"));
```

### License

Apache 2.0
