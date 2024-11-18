# Transaction types

Over time, the Ethereum network has undergone various upgrades and improvements to enhance transaction efficiency, security, and user experience. Four significant transaction types that have evolved are:

- Legacy Transactions,
- EIP-2930 Transactions,
- EIP-1559 Transactions,
- EIP-4844 Transactions

Each of these transaction types brings unique features and improvements to the Ethereum network.

## Legacy Transactions

Legacy Transactions (type `0x0`), the traditional Ethereum transactions in use since the network's inception, include the following parameters:
- `nonce`,
- `gasPrice`,
- `gasLimit`,
- `to`,
- `value`,
- `data`,
- `v`,
- `r`,
- `s`.

These transactions do not utilize access lists, which specify the addresses and storage keys to be accessed, nor do they incorporate EIP-1559 fee market changes.

## EIP-2930 Transactions

Introduced in [EIP-2930](https://eips.ethereum.org/EIPS/eip-2930), transactions with type `0x1` incorporate an `accessList` parameter alongside legacy parameters. This `accessList` specifies an array of addresses and storage keys that the transaction plans to access, enabling gas savings on cross-contract calls by pre-declaring the accessed contract and storage slots. They do not include EIP-1559 fee market changes.

## EIP-1559 Transactions

[EIP-1559](https://eips.ethereum.org/EIPS/eip-1559) transactions (type `0x2`) were introduced in Ethereum's London fork to address network congestion and transaction fee overpricing caused by the historical fee market. Unlike traditional transactions, EIP-1559 transactions don't specify a gas price (`gasPrice`). Instead, they use an in-protocol, dynamically changing base fee per gas, adjusted at each block to manage network congestion.

Alongside the `accessList` parameter and legacy parameters (except `gasPrice`), EIP-1559 transactions include:
- `maxPriorityFeePerGas`, specifying the maximum fee above the base fee the sender is willing to pay,
- `maxFeePerGas`, setting the maximum total fee the sender is willing to pay.

The base fee is burned, while the priority fee is paid to the miner who includes the transaction, incentivizing miners to include transactions with higher priority fees per gas.

## EIP-4844 Transactions

[EIP-4844](https://eips.ethereum.org/EIPS/eip-4844) transactions (type `0x3`) was introduced in Ethereum's Dencun fork. This provides a temporary but significant scaling relief for rollups by allowing them to initially scale to 0.375 MB per slot, with a separate fee market allowing fees to be very low while usage of this system is limited.

Alongside the legacy parameters & parameters from EIP-1559, the EIP-4844 transactions include:
- `max_fee_per_blob_gas`, The maximum total fee per gas the sender is willing to pay for blob gas in wei
- `blob_versioned_hashes`, List of versioned blob hashes associated with the transaction's EIP-4844 data blobs.

The actual blob fee is deducted from the sender balance before transaction execution and burned, and is not refunded in case of transaction failure.
