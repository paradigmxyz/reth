//! Hot addresses that frequently change state and should not be cached.
//!
//! These addresses account for the majority of cache validation failures in production.
//! Transactions involving these contracts are excluded from caching to avoid the overhead
//! of validation that will almost always fail.

use alloy_primitives::{address, Address};

/// Hot addresses that change frequently and should not be cached.
/// These account for 70%+ of validation failures, primarily WETH and stablecoins.
pub const HOT_ADDRESSES: &[Address] = &[
    address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"), // WETH (Mainnet)
    address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"), // USDC (Mainnet)
    address!("dAC17F958D2ee523a2206206994597C13D831ec7"), // USDT (Mainnet)
    address!("68b3465833fb72A70ecDF485E0e4C7bD8665Fc45"), // Uniswap V3 Router
    address!("E592427A0AEce92De3Edee1F18E0157C05861564"), // Uniswap V3 Router 2 (SwapRouter)
    address!("7a250d5630B4cF539739dF2C5dAcb4c659F2488D"), // Uniswap V2 Router
    address!("00000000219ab540356cBB839Cbe05303d7705Fa"), // Beacon Deposit Contract
];
