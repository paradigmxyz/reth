# TODO – Full ETH/68 Wire-Protocol Support (BSC)

The current `parlia-8e0ff9` branch intentionally runs on ETH/66 only in
order to avoid BSC-specific extensions that break the stock decoders.

Enhancement steps (when time permits):

1. Re-introduce tolerant RLP decoders for the following messages:
   • `NewBlockHashes` / `NewBlock` (4-field BSC extension)
   • `NewPooledTransactionHashes` (ETH/66 & ETH/68 variants)
   • Any future BSC-specific broadcast messages
2. Add unit tests using captured payloads from the official BSC client
   to guarantee compatibility.
3. Expose a CLI flag (or feature-flag) to opt-in for ETH/68 when
   peers offer it.
4. Monitor the public network: when the majority of BSC nodes migrate
   to ETH/68-only, switch the default to ETH/68. 