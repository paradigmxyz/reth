---
title: '`debug_traceCallMany` doesn''t return the same result as `debug_traceTransaction`'
labels:
    - C-bug
    - M-prevent-stale
    - S-needs-investigation
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.978521Z
info:
    author: vaigay
    created_at: 2024-11-21T06:39:20Z
    updated_at: 2025-01-03T09:55:11Z
---

### Describe the bug

Hi,

- When I retrieve all transactions from a minted block and add them to a bundle, set the `StateContext` to the previous block, set the `BlockOverrides` to the header information of the minted block
- Then call `debug_traceCallMany` with the `tracer` set to `callTracer`, I then call `debug_Transaction` with a specific transaction hash to compare the results. 
- Not all transactions are different; some of them are still correct. For example, some transactions have different results, such as transaction hash: `0x848bf3d087bcc53205c66a04b5d706d5bc2be0be0472d4afa9d4481eca8edef5`.

### Steps to reproduce

 - Because the request and response for `debug_traceCallMany` is too long so I wrote a test for this case, you can update `rpcUrl` and run the test 
```
package test

import (
	"context"
	"math/big"
	"os"
	"testing"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/common/hexutil"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/crypto/kzg4844"
	"github.com/ethereum/go-ethereum/ethclient"
	"github.com/ethereum/go-ethereum/rpc"
	"github.com/google/go-cmp/cmp"
	"github.com/stretchr/testify/assert"
)

type CallFrame struct {
	Type         string      `json:"type"`
	From         string      `json:"from"`
	Gas          string      `json:"gas"`
	GasUsed      string      `json:"gasUsed"`
	To           string      `json:"to,omitempty"`
	Input        string      `json:"input"`
	Output       string      `json:"output,omitempty"`
	Error        string      `json:"error,omitempty"`
	RevertReason string      `json:"revertReason,omitempty"`
	Calls        []CallFrame `json:"calls,omitempty"`
	Logs         []CallLog   `json:"logs,omitempty"`
	Value        string      `json:"value,omitempty"`
}

type CallLog struct {
	Address string   `json:"address"`
	Topics  []string `json:"topics"`
	Data    string   `json:"data"`
}

type TransactionSimulationRequest struct {
	From                 *common.Address   `json:"from,omitempty"`
	To                   *common.Address   `json:"to,omitempty"`
	Gas                  *hexutil.Uint64   `json:"gas,omitempty"`
	GasPrice             *hexutil.Big      `json:"gasPrice,omitempty"`
	MaxFeePerGas         *hexutil.Big      `json:"maxFeePerGas,omitempty"`
	MaxPriorityFeePerGas *hexutil.Big      `json:"maxPriorityFeePerGas,omitempty"`
	Value                *hexutil.Big      `json:"value,omitempty"`
	Nonce                *hexutil.Uint64   `json:"nonce,omitempty"`
	Data                 *hexutil.Bytes    `json:"data,omitempty"`
	AccessList           *types.AccessList `json:"accessList,omitempty"`
	BlobFeeCap           *hexutil.Big      `json:"maxFeePerBlobGas"`
	BlobHashes           []common.Hash     `json:"blobVersionedHashes,omitempty"`
	Sidecar              *Sidecar          `json:"sidecar,omitempty"`
	TransactionType      uint8             `json:"transactionType"`
}

type Sidecar struct {
	Blobs       []kzg4844.Blob       `json:"blobs,omitempty"`
	Commitments []kzg4844.Commitment `json:"commitments,omitempty"`
	Proofs      []kzg4844.Proof      `json:"proofs,omitempty"`
}

type BlockOverrides struct {
	Number     *hexutil.Big    `json:"number,omitempty"`
	Difficulty *hexutil.Uint64 `json:"difficulty,omitempty"`
	Time       *hexutil.Uint64 `json:"time,omitempty"`
	GasLimit   *hexutil.Uint64 `json:"gasLimit,omitempty"`
	Coinbase   *common.Address `json:"coinbase,omitempty"`
	Random     *common.Hash    `json:"random,omitempty"`
	BaseFee    *hexutil.Big    `json:"baseFee,omitempty"`
}

type Bundle struct {
	Transactions  []TransactionSimulationRequest `json:"transactions"`
	BlockOverride *BlockOverrides                `json:"blockOverride"`
}

type StateContext struct {
	BlockNumber rpc.BlockNumberOrHash `json:"blockNumber"`
}

func GetFrom(tx *types.Transaction) (common.Address, error) {
	chainId := tx.ChainId()
	if chainId == nil {
		chainId = new(big.Int).SetUint64(1)
	}

	from, err := types.Sender(types.LatestSignerForChainID(chainId), tx)
	return from, err
}

func convertPayload(tx *types.Transaction) TransactionSimulationRequest {
	from, _ := GetFrom(tx)
	gas := hexutil.Uint64(tx.Gas())
	nonce := hexutil.Uint64(tx.Nonce())
	byteData := hexutil.Bytes(tx.Data())
	gasPrice := (*hexutil.Big)(tx.GasPrice())
	maxFeePerGas := (*hexutil.Big)(tx.GasFeeCap())
	maxPriorityFeePerGas := (*hexutil.Big)(tx.GasTipCap())
	accessList := tx.AccessList()
	transactionRequest := TransactionSimulationRequest{
		From:            &from,
		To:              tx.To(),
		Gas:             &gas,
		Value:           (*hexutil.Big)(tx.Value()),
		Nonce:           &nonce,
		Data:            &byteData,
		AccessList:      &accessList,
		BlobFeeCap:      (*hexutil.Big)(tx.BlobGasFeeCap()),
		TransactionType: tx.Type(),
	}

	if int(tx.Type()) == types.BlobTxType {
		transactionRequest.BlobHashes = tx.BlobHashes()
		if tx.BlobTxSidecar() != nil {
			transactionRequest.Sidecar = &Sidecar{
				Blobs:       tx.BlobTxSidecar().Blobs,
				Commitments: tx.BlobTxSidecar().Commitments,
				Proofs:      tx.BlobTxSidecar().Proofs,
			}
		}
	}
	if IsSupportEIP1559(tx.Type()) {
		transactionRequest.MaxPriorityFeePerGas = maxPriorityFeePerGas
		transactionRequest.MaxFeePerGas = maxFeePerGas
	} else {
		transactionRequest.GasPrice = gasPrice
	}
	return transactionRequest
}

func IsSupportEIP1559(txType uint8) bool {
	switch txType {
	case types.LegacyTxType, types.AccessListTxType:
		return false
	case types.DynamicFeeTxType, types.BlobTxType:
		return true
	}
	return false
}

func DebugTraceCallManyCallFrame(ctx context.Context, rpcClient *rpc.Client, transactions types.Transactions, header *types.Header) ([]CallFrame, error) {
	if len(transactions) == 0 {
		return []CallFrame{}, nil
	}
	traceCallConfig := map[string]interface{}{
		"tracer": "callTracer",
		"tracerConfig": map[string]interface{}{
			"withLog": true,
		},
	}
	bundle, stateContext := prepareBundle(header, transactions)
	var result [][]CallFrame
	err := rpcClient.CallContext(ctx, &result, "debug_traceCallMany", []Bundle{bundle}, stateContext, traceCallConfig)
	if len(result) == 0 {
		return []CallFrame{}, err
	}
	return result[0], err
}

func prepareBundle(header *types.Header, transactions types.Transactions) (Bundle, StateContext) {
	h := header.ParentHash
	blockSimulated := rpc.BlockNumberOrHash{
		BlockHash: &h,
	}
	blockTime := (hexutil.Uint64)(header.Time)
	gasLimit := (hexutil.Uint64)(header.GasLimit)
	coinBase := header.Coinbase
	difficulty := (hexutil.Uint64)(header.Difficulty.Uint64())
	blockOverrides := BlockOverrides{
		Number:     (*hexutil.Big)(header.Number),
		Difficulty: &difficulty,
		Time:       &blockTime,
		GasLimit:   &gasLimit,
		Coinbase:   &coinBase,
		BaseFee:    (*hexutil.Big)(header.BaseFee),
	}

	var bundle Bundle
	var txs []TransactionSimulationRequest
	for _, tx := range transactions {
		txs = append(txs, convertPayload(tx))
	}
	bundle.Transactions = txs
	bundle.BlockOverride = &blockOverrides
	stateContext := StateContext{
		BlockNumber: blockSimulated,
	}
	return bundle, stateContext
}

func DebugTraceTransaction(ctx context.Context, rpcClient *rpc.Client, txHash string) (CallFrame, error) {
	var result CallFrame
	traceCallConfig := map[string]interface{}{
		"tracer": "callTracer",
		"tracerConfig": map[string]interface{}{
			"withLog": true,
		},
	}
	err := rpcClient.CallContext(ctx, &result, "debug_traceTransaction", txHash, traceCallConfig)
	return result, err
}

var rpcUrl = os.Getenv("RPC_URL")

func Test_debugTraceCallManyAndDebugTransaction(t *testing.T) {
	if rpcUrl == "" {
		t.Skip("RPC_URL is not set")
	}
	ctx := context.Background()
	ethClient, err := ethclient.Dial(rpcUrl)
	if err != nil {
		t.Fatal(err)
	}
	txHash := common.HexToHash("0x848bf3d087bcc53205c66a04b5d706d5bc2be0be0472d4afa9d4481eca8edef5")
	receipt, err := ethClient.TransactionReceipt(ctx, txHash)
	if err != nil {
		t.Fatal(err)
	}
	block, err := ethClient.BlockByHash(ctx, receipt.BlockHash)
	if err != nil {
		t.Fatal(err)
	}
	txIndex := -1
	for i, tx := range block.Transactions() {
		if tx.Hash() == txHash {
			txIndex = i
		}
	}
	assert.NotEqual(t, -1, txIndex, "transaction should be found in block")
	result, err := DebugTraceCallManyCallFrame(ctx, ethClient.Client(), block.Transactions(), block.Header())
	if err != nil {
		t.Fatal(err)
	}
	assert.Equal(t, len(block.Transactions()), len(result), "length of result should be equal to length of transactions")
	expectedResult, err := DebugTraceTransaction(ctx, ethClient.Client(), txHash.Hex())
	if err != nil {
		t.Fatal(err)
	}
	traceCallResult := result[txIndex]
	assert.True(t, cmp.Equal(traceCallResult, expectedResult), "debug_Transaction should equal to debug_traceCallMany")
}

```

### Node logs

```text

```

### Platform(s)

Linux (x86)

### What version/commit are you on?

 reth v1.1.2: 496bf0b 

### What database version are you on?

NA

### Which chain / network are you on?

mainnet

### What type of node are you running?

Full via --full flag

### What prune config do you use, if any?

_No response_

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [x] I agree to follow the Code of Conduct
