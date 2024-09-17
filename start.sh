#!/bin/bash

INSTALL_ROOT="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
OG_DIR="$(pwd)"
RETH_BIN_PATH=$INSTALL_ROOT/target/release/telos-reth

if [ ! -f $RETH_BIN_PATH ] || [ ! -x $RETH_BIN_PATH ]; then
    echo "Error: telos-reth binary not found at $RETH_BIN_PATH\nHint: Did you run build.sh yet?"
    exit 1
fi

cd $INSTALL_ROOT

[ -f $INSTALL_ROOT/.env ] && . $INSTALL_ROOT/.env

[ -z "$LOG_LEVEL" ] && LOG_LEVEL=info
[ -z "$DATA_DIR" ] && DATA_DIR=$INSTALL_ROOT/data
[ -z "$CHAIN" ] && CHAIN=tevmmainnet
[ -z "$RETH_CONFIG" ] && RETH_CONFIG=$DATA_DIR/config.toml
[ -z "$RETH_RPC_ADDRESS" ] && RETH_RPC_ADDRESS=
[ -z "$RETH_RPC_PORT" ] && RETH_RPC_PORT=8545
[ -z "$LOG_PATH" ] && LOG_PATH=$DATA_DIR/reth.log

nohup $RETH_BIN_PATH node \
        --log.stdout.filter $LOG_LEVEL \
        --datadir $DATA_DIR \
        --chain $CHAIN \
        --config $RETH_CONFIG \
        --http \
        --http.addr $RETH_RPC_ADDRESS \
        --http.port $RETH_RPC_PORT \
        --http.api all \
        --ws \
        --ws.api all \
        --telos.telos_endpoint $TELOS_ENDPOINT \
        --telos.signer_account $TELOS_SIGNER_ACCOUNT \
        --telos.signer_permission $TELOS_SIGNER_PERMISSION \
        --telos.signer_key $TELOS_SIGNER_KEY \
    >> "$LOG_PATH" 2>&1 &
PID="$!"
echo $PID > $INSTALL_ROOT/reth.pid

cd $OG_DIR