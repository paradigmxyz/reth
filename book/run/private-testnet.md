# Run Reth in a private testnet using Kurtosis
For those who need a private testnet to validate functionality or scale with Reth.

## Using Docker locally
This guide uses [Kurtosis' ethereum-package](https://github.com/kurtosis-tech/ethereum-package) and assumes you have Kurtosis and Docker installed and have Docker already running on your machine. 
* Go [here](https://docs.kurtosis.com/install/) to install Kurtosis
* Go [here](https://docs.docker.com/get-docker/) to install Docker

The [`ethereum-package`](https://github.com/kurtosis-tech/ethereum-package) is a [package](https://docs.kurtosis.com/concepts-reference/packages) for a general purpose Ethereum testnet definition used for instantiating private testnets at any scale over Docker or Kubernetes, locally or in the cloud. This guide will go through how to spin up a local private testnet with Reth various CL clients locally. Specifically, you will instantiate a 2-node network over Docker with Reth/Lighthouse and Reth/Teku client combinations.

To see all possible configurations and flags you can use, including metrics and observability tools (e.g. Grafana, Prometheus, etc), go [here](https://github.com/kurtosis-tech/ethereum-package#configuration).

Genesis data will be generated using this [genesis-generator](https://github.com/ethpandaops/ethereum-genesis-generator) to be used to bootstrap the EL and CL clients for each node. The end result will be a private testnet with nodes deployed as Docker containers in an ephemeral, isolated environment on your machine called an [enclave](https://docs.kurtosis.com/concepts-reference/enclaves/). Read more about how the `ethereum-package` works by going [here](https://github.com/kurtosis-tech/ethereum-package/).

### Step 1: Define the parameters and shape of your private network
First, in your home directory, create a file with the name `network_params.json` with the following contents:
```json
{
  "participants": [
    {
      "el_client_type": "reth",
      "el_client_image": "ghcr.io/paradigmxyz/reth",
      "cl_client_type": "lighthouse",
      "cl_client_image": "sigp/lighthouse:latest",
      "count": 1
    },
    {
      "el_client_type": "reth",
      "el_client_image": "ghcr.io/paradigmxyz/reth",
      "cl_client_type": "teku",
      "cl_client_image": "consensys/teku:latest",
      "count": 1
    }
  ],
  "launch_additional_services": false
}
```
### Step 2: Spin up your network

Next, run the following command from your command line:
```bash
kurtosis run github.com/kurtosis-tech/ethereum-package --args-file ~/network_params.json
```
Kurtosis will spin up an [enclave](https://docs.kurtosis.com/concepts-reference/enclaves) (i.e an ephemeral, isolated environment) and begin to configure and instantiate the nodes in your network. In the end, Kurtosis will print the services running in your enclave that form your private testnet alongside all the container ports and files that were generated & used to start up the private testnet. Here is a sample output:
```console
INFO[2023-08-21T18:22:18-04:00] ====================================================
INFO[2023-08-21T18:22:18-04:00] ||          Created enclave: silky-swamp          ||
INFO[2023-08-21T18:22:18-04:00] ====================================================
Name:            silky-swamp
UUID:            3df730c66123
Status:          RUNNING
Creation Time:   Mon, 21 Aug 2023 18:21:32 EDT

========================================= Files Artifacts =========================================
UUID           Name
c168ec4468f6   1-lighthouse-reth-0-63
61f821e2cfd5   2-teku-reth-64-127
e6f94fdac1b8   cl-genesis-data
e6b57828d099   el-genesis-data
1fb632573a2e   genesis-generation-config-cl
b8917e497980   genesis-generation-config-el
6fd8c5be336a   geth-prefunded-keys
6ab83723b4bd   prysm-password

========================================== User Services ==========================================
UUID           Name                                       Ports                                         Status
95386198d3f9   cl-1-lighthouse-reth                       http: 4000/tcp -> http://127.0.0.1:64947      RUNNING
                                                          metrics: 5054/tcp -> http://127.0.0.1:64948
                                                          tcp-discovery: 9000/tcp -> 127.0.0.1:64949
                                                          udp-discovery: 9000/udp -> 127.0.0.1:60303
5f5cc4cf639a   cl-1-lighthouse-reth-validator             http: 5042/tcp -> 127.0.0.1:64950             RUNNING
                                                          metrics: 5064/tcp -> http://127.0.0.1:64951
27e1cfaddc72   cl-2-teku-reth                             http: 4000/tcp -> 127.0.0.1:64954             RUNNING
                                                          metrics: 8008/tcp -> 127.0.0.1:64952
                                                          tcp-discovery: 9000/tcp -> 127.0.0.1:64953
                                                          udp-discovery: 9000/udp -> 127.0.0.1:53749
b454497fbec8   el-1-reth-lighthouse                       engine-rpc: 8551/tcp -> 127.0.0.1:64941       RUNNING
                                                          metrics: 9001/tcp -> 127.0.0.1:64937
                                                          rpc: 8545/tcp -> 127.0.0.1:64939
                                                          tcp-discovery: 30303/tcp -> 127.0.0.1:64938
                                                          udp-discovery: 30303/udp -> 127.0.0.1:55861
                                                          ws: 8546/tcp -> 127.0.0.1:64940
03a2ef13c99b   el-2-reth-teku                             engine-rpc: 8551/tcp -> 127.0.0.1:64945       RUNNING
                                                          metrics: 9001/tcp -> 127.0.0.1:64946
                                                          rpc: 8545/tcp -> 127.0.0.1:64943
                                                          tcp-discovery: 30303/tcp -> 127.0.0.1:64942
                                                          udp-discovery: 30303/udp -> 127.0.0.1:64186
                                                          ws: 8546/tcp -> 127.0.0.1:64944
5c199b334236   prelaunch-data-generator-cl-genesis-data   <none>                                        RUNNING
46829c4bd8b0   prelaunch-data-generator-el-genesis-data   <none>                                        RUNNING
```

Great! You now have a private network with 2 full Ethereum nodes on your local machine over Docker - one that is a Reth/Lighthouse pair and another that is Reth/Teku. Check out the [Kurtosis docs](https://docs.kurtosis.com/cli) to learn about the various ways you can interact with and inspect your network. 

## Using Kurtosis on Kubernetes
Kurtosis packages are portable and reproducible, meaning they will work the same way over Docker or Kubernetes, locally or on remote infrastructure. For use cases that require a larger scale, Kurtosis can be deployed on Kubernetes by following these docs [here](https://docs.kurtosis.com/k8s/).

## Running the network with additional services
The [`ethereum-package`](https://github.com/kurtosis-tech/ethereum-package) comes with many optional flags and arguments you can enable for your private network. Some include:
- A Grafana + Prometheus instance
- A transaction spammer called [`tx-fuzz`](https://github.com/MariusVanDerWijden/tx-fuzz)
- [A network metrics collector](https://github.com/dapplion/beacon-metrics-gazer)
- Flashbot's `mev-boost` implementation of PBS (to test/simulate MEV workflows)

### Questions?
Please reach out to the [Kurtosis discord](https://discord.com/invite/6Jjp9c89z9) should you have any questions about how to use the `ethereum-package` for your private testnet needs. Thanks!
