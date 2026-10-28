EIP-8253 uses a chain-specific `config.zeroNonceStorageAccounts` address list.
Mainnet defaults to the published 28 addresses; public testnets and custom chains
default to an empty list. Custom chains carrying Mainnet state must configure the
list explicitly. A shared chain ID alone does not select the Mainnet list.

The legacy `eip8253Accounts` name is accepted on input; serialization uses
`zeroNonceStorageAccounts`. An explicit empty list disables the nonce bump,
including the Mainnet default.

`bogota-eip8253-alloc.json` contains the official Mainnet prestate allocation from
[`ethereum/EIPs@fb527daa`](https://github.com/ethereum/EIPs/tree/fb527daa1830467d6be1336cbf36ed11ef23e6a2/assets/eip-8253).
Devnets testing the transition should merge these accounts into genesis `alloc`,
set `zeroNonceStorageAccounts` to the fragment's address keys, and schedule
`bogotaTime` after genesis. Preserve the zero nonces and all 129 storage entries in
the fragment. Networks with different state must derive their own list.

At the Bogota boundary, before system calls, execution sets each configured
account's nonce to one and records the nonce change at BAL index zero. Balance,
code and storage remain unchanged. Activation at genesis has no fork transition.
