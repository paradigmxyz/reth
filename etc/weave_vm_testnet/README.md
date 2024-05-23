# Reth custom testnet Deployment Guide

## 1. Create a VM and SSH to it:

ssh to your vm

## 2. Create User `reth-node`
```bash
sudo adduser reth-node
sudo usermod -aG sudo reth-node
sudo apt update
```

## 3. Install Docker
### Add Docker's Official GPG Key:
```bash
sudo apt-get update
sudo apt-get install ca-certificates curl
sudo install -m 0755 -d /etc/apt/keyrings
sudo curl -fsSL https://download.docker.com/linux/ubuntu/gpg -o /etc/apt/keyrings/docker.asc
sudo chmod a+r /etc/apt/keyrings/docker.asc
```

### Add the Repository to Apt Sources:
```bash
echo   "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/ubuntu   $(. /etc/os-release && echo "$VERSION_CODENAME") stable" |   sudo tee /etc/apt/sources.list.d/docker.list > /dev/null
sudo apt-get update
sudo apt-get install docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
```

## 4. Install Rust
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
apt-get install libclang-dev pkg-config build-essential
```

## 5. Get Sources
```bash
mkdir code
git clone https://github.com/weaveVM/wvm-reth.git
```

## 6. Build Reth Local Docker Image
```bash
docker build . -t reth:local
```

## 7. Prepare for Genesis Configuration
```bash
sudo usermod -aG docker reth-node
sudo systemctl restart docker
newgrp docker
```

### Navigate to Directory
```bash
cd /code/wvm-reth/etc/weave_vm_testnet
docker compose up --no-start
```

### Copy Genesis Files
```bash
cd /code/testnet/
cp /home/reth-node/code/wvm-reth/crates/primitives/res/genesis/weave_wm_testnet_v0.json genesis.json
cp /home/reth-node/code/wvm-reth/etc/weave_vm_testnet/prysm_config/config.yml .
```

## 8. Install Go and Compile `prysmctl`
```bash
wget https://go.dev/dl/go1.22.3.linux-amd64.tar.gz
sudo tar -C /usr/local -xzf go1.22.3.linux-amd64.tar.gz
export PATH=$PATH:/usr/local/go/bin
source ~/.bashrc

cd code/
git clone https://github.com/prysmaticlabs/prysm && cd prysm
CGO_CFLAGS="-O2 -D__BLST_PORTABLE__" go build -o=../prysmctl ./cmd/prysmctl
```

### Generate Genesis
```bash
./prysmctl testnet generate-genesis --fork capella     --num-validators 1     --genesis-time-delay 600     --chain-config-file /home/reth-node/testnet/config.yml     --geth-genesis-json-in /home/reth-node/testnet/genesis.json     --geth-genesis-json-out /home/reth-node/testnet/genesis.json     --output-ssz /home/reth-node/testnet/genesis.ssz
```

### Move Generated Files to Docker Volumes
```bash
sudo cp config.yml /var/lib/docker/volumes/weave_vm_testnet_prysm_data/_data/config.yml
sudo cp genesis.ssz /var/lib/docker/volumes/weave_vm_testnet_prysm_data/_data/genesis.ssz
sudo cp genesis.json /var/lib/docker/volumes/weave_vm_testnet_reth_genesis/_data/genesis.json
```

## 9. Configure Firewall on VM and Cloud
```bash
gcloud compute --project=promising-rock-414216 firewall-rules create eth-monitoring --description="ports open for monitoring" --direction=INGRESS --priority=1000 --network=default --action=ALLOW --rules=tcp:3000
gcloud compute --project=promising-rock-414216 firewall-rules create eth-ports --description="ports open for monitoring and metrics" --direction=INGRESS --priority=1000 --network=default --action=ALLOW --rules=tcp:3000,9090
```

## 10. Configure Firewall on VM
### Install UFW and Allow Ports
```bash
sudo apt update
sudo apt install ufw
sudo ufw allow ssh
sudo ufw allow 22/tcp
sudo ufw allow 4000/tcp
sudo ufw allow 3500/tcp
sudo ufw allow 8080/tcp
sudo ufw allow 6060/tcp
sudo ufw allow 9090/tcp
sudo ufw allow 8551/tcp
sudo ufw allow 8545/tcp
sudo ufw allow 8546/tcp
sudo ufw enable
sudo ufw status
```

## 11. Generate JWT Token
you may find a script inside the node dir in /etc
```bash
sudo ./generate-jwt.sh
```

## 12. Initialize Reth Genesis
```bash
sudo /home/reth-node/code/wvm-reth/target/release/reth init --chain=/var/lib/docker/volumes/weave_vm_testnet_reth_genesis/_data/genesis.json --datadir=/var/lib/docker/volumes/weave_vm_testnet_reth_data/_data
```

## 13. Run Consensus Client, Execution Client, and Monitoring
```bash
cd /code/wvm-reth/etc/weave_vm_testnet
docker compose up -d
```

### some blueprint commands
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
      --suggested-fee-recipient 0xa2A0D977847805fE224B789D8C4d3D711ab251e7 \
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
        --suggested-fee-recipient 0xa2A0D977847805fE224B789D8C4d3D711ab251e7 \
        --minimum-peers-per-subnet 0 \
        --enable-debug-rpc-endpoints
```


2. Validator

```
./validator --datadir validatordata --accept-terms-of-use --interop-num-validators 1 --chain-config-file /home/reth-node/code/wvm-reth/etc/weave_vm_testnet/prysm_config/config.yml
```
