##

0. generate genesis
```
./prysmctl testnet generate-genesis --fork capella \
    --num-validators 1 \
    --genesis-time-delay 600 \
    --chain-config-file /home/reth-node/testnet/config.yml \
    --geth-genesis-json-in /home/reth-node/testnet/genesis.json  \
    --geth-genesis-json-out /home/reth-node/testnet/genesis.json \
    --output-ssz /home/reth-node/testnet/genesis.ssz
```

0.1
sudo /home/reth-node/code/wvm-reth/target/release/reth init  --chain=/var/lib/docker/volumes/weave_vm_testnet_reth_genesis/_data/genesis.json --datadir=/var/lib/docker/volumes/weave_vm_testnet_reth_data/_data

1. beacon-chain command
```
./beacon-chain --datadir beacondata \
  --min-sync-peers 0 \
  --genesis-state /home/reth-node/testnet/genesis.ssz \
    --bootstrap-node= --interop-eth1data-votes \
     --chain-config-file /home/reth-node/code/wvm-reth/etc/weave_vm_testnet/prysm_config/config.yml \
     --contract-deployment-block 0 \
      --chain-id ${CHAIN_ID:-9496} \
      --execution-endpoint http://0.0.0.0:8551 \
      --accept-terms-of-use \
      --jwt-secret /home/reth-node/code/wvm-reth/etc/jwttoken/jwt.hex \
      --suggested-fee-recipient 0x123463a4B065722E99115D6c222f267d9cABb524 \
      --minimum-peers-per-subnet 0 \
      --enable-debug-rpc-endpoints
```

or 
```
sudo ./beacon-chain --datadir beacondata \
    --min-sync-peers 0 \
    --genesis-state /home/reth-node/testnet/genesis.ssz \
    --bootstrap-node= --interop-eth1data-votes \
        --chain-config-file /home/reth-node/code/wvm-reth/etc/weave_vm_testnet/prysm_config/config.yml \
        --contract-deployment-block 0 \
        --chain-id ${CHAIN_ID:-9496} \
        --execution-endpoint /var/lib/docker/volumes/weave_vm_testnet_reth_data/_data/reth_engine_api.ipc \
        --accept-terms-of-use \
        --jwt-secret /home/reth-node/code/wvm-reth/etc/jwttoken/jwt.hex \
        --suggested-fee-recipient 0x123463a4B065722E99115D6c222f267d9cABb524 \
        --minimum-peers-per-subnet 0 \
        --enable-debug-rpc-endpoints
```


2. Validator

```
./validator --datadir validatordata --accept-terms-of-use --interop-num-validators 1 --chain-config-file /home/reth-node/code/wvm-reth/etc/weave_vm_testnet/prysm_config/config.yml
```
