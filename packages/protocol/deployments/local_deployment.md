# How to deploy Gwyneth locally - on a reth-based private network

The first part is coming from [Reth Book](https://reth.rs/run/private-testnet.html), but if you want to dig deeper, please visit the website, otherwise it is not necessary.

### 0. Pre-requisites:
- have docker installed (and docker daemon running)
- have Kurtosis installed, on Mac, e.g.:
```shell
brew install kurtosis-tech/tap/kurtosis-cli
```

### 1. Define the network config parameters

Create a `network_param.yaml` file.

```shell
participants:
  - el_type: reth
    el_image: ghcr.io/paradigmxyz/reth # We can use custom image, (remote, e.g.: ethpandaops/reth:main-9c0bc84 or locally: taiko_reth)
    cl_type: lighthouse
    cl_image: sigp/lighthouse:latest
  - el_type: reth
    el_image: ghcr.io/paradigmxyz/reth # We can use custom image, (remote, e.g.: ethpandaops/reth:main-9c0bc84 or locally: taiko_reth)
    cl_type: teku
    cl_image: consensys/teku:latest
```

#### 1.1 Local reth-based network

1. Go to the root of the repository, and build the image, e.g.:
```shell
docker build . -t taiko_reth
```

2. Use simply the `taiko_reth` image, in `el_image` variable of the network yaml file.

### 2. Spin up the network

```shell
kurtosis run github.com/ethpandaops/ethereum-package --args-file YOUR_NETWORK_FILE_PATH/network_params.yaml
```

It will show you a lot of information in the terminal - along with the genesis info, network id, addresses with pre-funded ETH, etc.

### 3. Set .env vars and run contract deployment script
Paste one PK and ADDR pair from anvil output to .env file and set the correct corresponding (PRIVATE_KEY and MAINNET_CONTRACT_OWNER) variables.

Run script:

```shell
$ forge script --rpc-url  http://127.0.0.1:52178 scripts/DeployL1Locally.s.sol -vvvv --broadcast --private-key <YOUR_PRIVATE_KEY> --legacy
```

Important: <YOUR_PRIVATE_KEY> shall be the same PK as you set in the ENV file.

### 4. Test interaction with the blockchain

Shoot it with simple RPC commands e.g. via `curl`, to see the blockchain is operational.

```shell
curl http://127.0.0.1:YOUR_EXPOSED_PORT \
  -X POST \
  -H "Content-Type: application/json" \
  --data '{"method":"eth_getBlockByNumber","params":["0x0",false],"id":1,"jsonrpc":"2.0"}'
```