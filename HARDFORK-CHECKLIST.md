# Non-exhaustive checklist for integrating new changes for an upcoming hard fork/devnet

## Introducing new EIP types or changes to primitive types

- Make required changes to primitive data structures on [alloy](https://github.com/alloy-rs/alloy)
- All new EIP data structures/constants/helpers etc. go into the `alloy-eips` crate at first.
- New transaction types go into `alloy-consensus`
- If there are changes to existing data structures, such as `Header` or `Block`, apply them to the types in
  `alloy-consensus` (e.g. new `request_hashes` field in Prague)

## Engine API

- If there are changes to the engine API (e.g. a new `engine_newPayloadVx` and `engine_getPayloadVx` pair) add the new
  types to the `alloy-rpc-types-engine` crate.
- If there are new parameters to the `engine_newPayloadVx` endpoint, add them to the `ExecutionPayloadSidecar` container
  type. This types contains all additional parameters that are required to convert an `ExecutionPayload` to an EL block.

## Reth changes

### Updates to the engine API

- Add new endpoints to the `EngineApi` trait and implement endpoints.
- Update the `ExecutionPayload` + `ExecutionPayloadSidecar` to `Block` conversion if there are any additional
  parameters.
- Update version specific validation checks in the `EngineValidator` trait.

## Op-Reth changes

### Updates to the engine API

Opstack tries to be as close to the L1 engine API as much as possible. Isthmus (Prague equivalent) introduced the first
deviation from the L1 engine API with an additional field in the `ExecutionPayload`. For this reason the op engine API
has its own server traits `OpEngineApi`.
Adding a new versioned endpoint requires the same changes as for L1 just for the dedicated OP types.

### Hardforks

Opstack has dedicated hardforks (e.g. Isthmus), that can be entirely opstack specific (e.g. Holocene) or can be an L1
equivalent hardfork. Since opstack sticks to the L1 header primitive, a new L1 equivalent hardfork also requires new
equivalent consensus checks. For this reason these `OpHardfork` must be mapped to L1 `EthereumHardfork`, for example:
`OpHardfork::Isthmus` corresponds to `EthereumHardfork::Prague`. These mappings must be defined in the `ChainSpec`.
