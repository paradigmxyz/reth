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
    el_image: ghcr.io/paradigmxyz/reth # We can use custom image, like ethpandaops/reth:main-9c0bc84 with MacOs with M1 chip or taiko.xyz/taiko-reth for example
    cl_type: lighthouse
    cl_image: sigp/lighthouse:latest
  - el_type: reth
    el_image: ghcr.io/paradigmxyz/reth # We can use custom image, like ethpandaops/reth:main-9c0bc84 with MacOs with M1 chip or taiko.xyz/taiko-reth for example
    cl_type: teku
    cl_image: consensys/teku:latest
```

### 2. Spin up the network

```shell
kurtosis run github.com/ethpandaops/ethereum-package --args-file YOUR_NETWORK_FILE/network_params.json
```

It will show you a lot og information in the terminal - along with the genesis info, network id, addresses with pre-funded ETH, etc.

### 3. Set .env vars and run deployment script
Paste one PK and ADDR pair from anvil output to .env file and set the correct corresponding (PRIVATE_KEY and MAINNET_CONTRACT_OWNER) variables.

Run script:

```shell
$ forge script --rpc-url  http://127.0.0.1:52178 scripts/DeployL1Locally.s.sol -vvvv --broadcast --private-key <YOUR_PRIVATE_KEY> --legacy
```

Important: <YOUR_PRIVATE_KEY> shall be the same PK as you set in the ENV file.