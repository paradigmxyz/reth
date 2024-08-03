# Run Reth in a private testnet using Kurtosis
For those who need a private testnet to validate functionality or scale with Reth.

## Using Docker locally
This guide uses [Kurtosis' ethereum-package](https://github.com/ethpandaops/ethereum-package) and assumes you have Kurtosis and Docker installed and have Docker already running on your machine.
* Go [here](https://docs.kurtosis.com/install/) to install Kurtosis
* Go [here](https://docs.docker.com/get-docker/) to install Docker

The [`ethereum-package`](https://github.com/ethpandaops/ethereum-package) is a [package](https://docs.kurtosis.com/advanced-concepts/packages) for a general purpose Ethereum testnet definition used for instantiating private testnets at any scale over Docker or Kubernetes, locally or in the cloud. This guide will go through how to spin up a local private testnet with Reth various CL clients locally. Specifically, you will instantiate a 2-node network over Docker with Reth/Lighthouse and Reth/Teku client combinations.

To see all possible configurations and flags you can use, including metrics and observability tools (e.g. Grafana, Prometheus, etc), go [here](https://github.com/ethpandaops/ethereum-package#configuration).

Genesis data will be generated using this [genesis-generator](https://github.com/ethpandaops/ethereum-genesis-generator) to be used to bootstrap the EL and CL clients for each node. The end result will be a private testnet with nodes deployed as Docker containers in an ephemeral, isolated environment on your machine called an [enclave](https://docs.kurtosis.com/advanced-concepts/enclaves/). Read more about how the `ethereum-package` works by going [here](https://github.com/ethpandaops/ethereum-package/).

### Step 1: Define the parameters and shape of your private network
First, in your home directory, create a file with the name `network_params.yaml` with the following contents:
```yaml
participants:
  - el_type: reth
    el_image: ghcr.io/paradigmxyz/reth
    cl_type: lighthouse
    cl_image: sigp/lighthouse:latest
  - el_type: reth
    el_image: ghcr.io/paradigmxyz/reth
    cl_type: teku
    cl_image: consensys/teku:latest
```

> [!TIP]
> If you would like to use a modified reth node, you can build an image locally with a custom tag. The tag can then be used in the `el_image` field in the `network_params.yaml` file.

### Step 2: Spin up your network

Next, run the following command from your command line:
```bash
kurtosis run github.com/ethpandaops/ethereum-package --args-file ~/network_params.yaml --image-download always
```
Kurtosis will spin up an [enclave](https://docs.kurtosis.com/advanced-concepts/enclaves/) (i.e an ephemeral, isolated environment) and begin to configure and instantiate the nodes in your network. In the end, Kurtosis will print the services running in your enclave that form your private testnet alongside all the container ports and files that were generated & used to start up the private testnet. Here is a sample output:
```console
INFO[2024-07-09T12:01:35+02:00] ========================================================
INFO[2024-07-09T12:01:35+02:00] ||          Created enclave: silent-mountain          ||
INFO[2024-07-09T12:01:35+02:00] ========================================================
Name:            silent-mountain
UUID:            cb5d0a7d0e7c
Status:          RUNNING
Creation Time:   Tue, 09 Jul 2024 12:00:03 CEST
Flags:

========================================= Files Artifacts =========================================
UUID           Name
414a075a37aa   1-lighthouse-reth-0-63-0
34d0b9ff906b   2-teku-reth-64-127-0
dffa1bcd1da1   el_cl_genesis_data
fdb202429b26   final-genesis-timestamp
da0d9d24b340   genesis-el-cl-env-file
55c46a6555ad   genesis_validators_root
ba79dbd109dd   jwt_file
04948fd8b1e3   keymanager_file
538211b6b7d7   prysm-password
ed75fe7d5293   validator-ranges

========================================== User Services ==========================================
UUID           Name                                             Ports                                         Status
0853f809c300   cl-1-lighthouse-reth                             http: 4000/tcp -> http://127.0.0.1:32811      RUNNING
                                                                metrics: 5054/tcp -> http://127.0.0.1:32812
                                                                tcp-discovery: 9000/tcp -> 127.0.0.1:32813
                                                                udp-discovery: 9000/udp -> 127.0.0.1:32776
f81cd467efe3   cl-2-teku-reth                                   http: 4000/tcp -> http://127.0.0.1:32814      RUNNING
                                                                metrics: 8008/tcp -> http://127.0.0.1:32815
                                                                tcp-discovery: 9000/tcp -> 127.0.0.1:32816
                                                                udp-discovery: 9000/udp -> 127.0.0.1:32777
f21d5ca3061f   el-1-reth-lighthouse                             engine-rpc: 8551/tcp -> 127.0.0.1:32803       RUNNING
                                                                metrics: 9001/tcp -> http://127.0.0.1:32804
                                                                rpc: 8545/tcp -> 127.0.0.1:32801
                                                                tcp-discovery: 30303/tcp -> 127.0.0.1:32805
                                                                udp-discovery: 30303/udp -> 127.0.0.1:32774
                                                                ws: 8546/tcp -> 127.0.0.1:32802
e234b3b4a440   el-2-reth-teku                                   engine-rpc: 8551/tcp -> 127.0.0.1:32808       RUNNING
                                                                metrics: 9001/tcp -> http://127.0.0.1:32809
                                                                rpc: 8545/tcp -> 127.0.0.1:32806
                                                                tcp-discovery: 30303/tcp -> 127.0.0.1:32810
                                                                udp-discovery: 30303/udp -> 127.0.0.1:32775
                                                                ws: 8546/tcp -> 127.0.0.1:32807
92dd5a0599dc   validator-key-generation-cl-validator-keystore   <none>                                        RUNNING
f0a7d5343346   vc-1-reth-lighthouse                             metrics: 8080/tcp -> http://127.0.0.1:32817   RUNNING
```

Great! You now have a private network with 2 full Ethereum nodes on your local machine over Docker - one that is a Reth/Lighthouse pair and another that is Reth/Teku. Check out the [Kurtosis docs](https://docs.kurtosis.com/cli) to learn about the various ways you can interact with and inspect your network.

## Using Kurtosis on Kubernetes
Kurtosis packages are portable and reproducible, meaning they will work the same way over Docker or Kubernetes, locally or on remote infrastructure. For use cases that require a larger scale, Kurtosis can be deployed on Kubernetes by following these docs [here](https://docs.kurtosis.com/k8s/).

## Running the network with additional services
The [`ethereum-package`](https://github.com/ethpandaops/ethereum-package) comes with many optional flags and arguments you can enable for your private network. Some include:
- A Grafana + Prometheus instance
- A transaction spammer called [`tx-fuzz`](https://github.com/MariusVanDerWijden/tx-fuzz)
- [A network metrics collector](https://github.com/dapplion/beacon-metrics-gazer)
- Flashbot's `mev-boost` implementation of PBS (to test/simulate MEV workflows)

### Questions?
Please reach out to the [Kurtosis discord](https://discord.com/invite/6Jjp9c89z9) should you have any questions about how to use the `ethereum-package` for your private testnet needs. Thanks!
