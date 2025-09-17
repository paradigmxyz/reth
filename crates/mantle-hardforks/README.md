# Mantle Hardforks

This crate provides hardfork definitions and utilities specific to the Mantle network, which is built on top of the OP Stack.

## Overview

Mantle is a Layer 2 scaling solution that extends the OP Stack with network-specific optimizations and features. This crate defines the hardfork progression for Mantle networks, including both standard OP Stack hardforks and Mantle-specific upgrades.

## Features

- **Mantle-specific hardforks**: Defines hardforks unique to the Mantle network
- **OP Stack compatibility**: Maintains full compatibility with the OP Stack hardfork system
- **Chain-specific configurations**: Separate configurations for Mantle mainnet and testnet
- **Timestamp-based activation**: Uses timestamp-based activation for network upgrades

## Hardforks

### Mantle-Specific Hardforks

- **Skadi**: Mantle's Prague-equivalent upgrade that introduces network-specific features and optimizations

### OP Stack Hardforks

Mantle inherits all OP Stack hardforks:
- Bedrock
- Regolith  
- Canyon
- Ecotone
- Fjord
- Granite
- Holocene
- Isthmus
- Interop

## Usage

```rust
use mantle_hardforks::{MantleHardfork, MantleChainHardforks, MANTLE_MAINNET_HARDFORKS};

// Check if Skadi is active at a given timestamp
let mantle_forks = MantleChainHardforks::mantle_mainnet();
let is_skadi_active = mantle_forks.is_skadi_active_at_timestamp(1_756_278_000);

// Get the full Mantle network hardforks configuration
let network_hardforks = MANTLE_MAINNET_HARDFORKS.clone();
```

## Chain IDs

- **Mantle Mainnet**: 5000
- **Mantle Sepolia**: 5001

## Activation Timestamps

- **Skadi**: 1756278000 (August 15, 2025 12:00:00 UTC)

## License

This project is licensed under the same terms as the main Reth project.
