# Indexed cell wire fixtures

Generated with geth's RLP package pinned to
`24b38e763fdd7dee8b91007d5f1a94cd21c9a7f0`. The packet structs in
`generate.go` follow EIP-8408 directly; no Reth or Alloy codec is used.
The generator also decodes and re-encodes every packet with geth.

In a temporary Go module directory, replace the generator path below with its
absolute path in this checkout, then run:

```sh
go mod init eip8408-vectors
go get github.com/ethereum/go-ethereum/rlp@24b38e763fdd7dee8b91007d5f1a94cd21c9a7f0
go run /path/to/generate.go > vectors.json
```

The fixtures cover heterogeneous little-endian masks, repeated mask identifiers,
a partial response with a reordered table and subset masks, and an empty indexed
response. The first byte is the eth message ID.

These are independent codec vectors, not evidence of a complete eth/73 peer
implementation in geth. Full client interoperability remains to be checked
before eth/73 is enabled by default.
