# historical status

- We released 1.0 "production-ready" stable Reth in June 2024.
  - Reth completed an audit with [Sigma Prime](https://sigmaprime.io/), the developers of [Lighthouse](https://github.com/sigp/lighthouse), the Rust Consensus Layer implementation. Find it [here](./audit/sigma_prime_audit_v2.pdf).
  - Revm (the EVM used in Reth) underwent an audit with [Guido Vranken](https://twitter.com/guidovranken) (#1 [Ethereum Bug Bounty](https://ethereum.org/en/bug-bounty)). We will publish the results soon.
- We released multiple iterative beta versions, up to [beta.9](https://github.com/paradigmxyz/reth/releases/tag/v0.2.0-beta.9) on Monday June 3rd 2024 the last beta release.
- We released [beta](https://github.com/paradigmxyz/reth/releases/tag/v0.2.0-beta.1) on Monday March 4th 2024, our first breaking change to the database model, providing faster query speed, smaller database footprint, and allowing "history" to be mounted on separate drives.
- We shipped iterative improvements until the last alpha release on February 28th 2024, [0.1.0-alpha.21](https://github.com/paradigmxyz/reth/releases/tag/v0.1.0-alpha.21).
- We [initially announced](https://www.paradigm.xyz/2023/06/reth-alpha) [0.1.0-alpha.1](https://github.com/paradigmxyz/reth/releases/tag/v0.1.0-alpha.1) in June 20th 2023.

## Database compatibility

We do not have any breaking database changes since beta.1, and do not plan any in the near future.

Reth [v0.2.0-beta.1](https://github.com/paradigmxyz/reth/releases/tag/v0.2.0-beta.1) includes
a [set of breaking database changes](https://github.com/paradigmxyz/reth/pull/5191) that makes it impossible to use database files produced by earlier versions.

If you had a database produced by alpha versions of Reth, you need to drop it with `reth db drop`
(using the same arguments such as `--config` or `--datadir` that you passed to `reth node`), and resync using the same `reth node` command you've used before.