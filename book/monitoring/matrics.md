# Matrics
Reth integrated with Prometheus allows users to measure various metrics to gain insight into the status and performance of a Reth node. For example, users can track the network status, transaction pool status, and other metrics to understand how the node is functioning.

## Stage Header

- **headers_counter**: Number of headers successfully retrieved
- **timeout_errors**: Number of timeout errors while requesting headers
- **validation_errors**: Number of validation errors while requesting headers
- **unexpected_errors**: Number of unexpected errors while requesting headers

## Transaction pool

- **inserted_transactions**: Number of transactions inserted in the pool
- **invalid_transactions**: Number of invalid transactions
- **removed_transactions**: Number of removed transactions from the pool