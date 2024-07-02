/// <reference types="node" />
/// <reference types="node" />
import { Block } from "@nomicfoundation/ethereumjs-block";
import { Common } from "@nomicfoundation/ethereumjs-common";
import { TypedTransaction } from "@nomicfoundation/ethereumjs-tx";
import { Address, ECDSASignature } from "@nomicfoundation/ethereumjs-util";
import EventEmitter from "events";
import { CompilerInput, CompilerOutput } from "../../../types";
import { RpcDebugTracingConfig } from "../../core/jsonrpc/types/input/debugTraceTransaction";
import { HardhatMetadata } from "../../core/jsonrpc/types/output/metadata";
import { HardforkName } from "../../util/hardforks";
import { MessageTrace } from "../stack-traces/message-trace";
import "./ethereumjs-workarounds";
import { StateOverrideSet } from "../../core/jsonrpc/types/input/callRequest";
import { CallParams, EstimateGasResult, FeeHistory, FilterParams, MineBlockResult, NodeConfig, RunCallResult, SendTransactionResult, TransactionParams } from "./node-types";
import { RpcLogOutput, RpcReceiptOutput } from "./output";
export declare class HardhatNode extends EventEmitter {
    private readonly _vm;
    private readonly _instanceId;
    private readonly _stateManager;
    private readonly _blockchain;
    private readonly _txPool;
    private _automine;
    private _minGasPrice;
    private _blockTimeOffsetSeconds;
    private _mempoolOrder;
    private _coinbase;
    private readonly _configNetworkId;
    private readonly _configChainId;
    readonly hardfork: HardforkName;
    private readonly _hardforkActivations;
    private _mixHashGenerator;
    readonly allowUnlimitedContractSize: boolean;
    private _allowBlocksWithSameTimestamp;
    private _forkNetworkId?;
    private _forkBlockNumber?;
    private _forkBlockHash?;
    private _forkClient?;
    private readonly _enableTransientStorage;
    static create(config: NodeConfig): Promise<[Common, HardhatNode]>;
    private static _validateHardforks;
    private readonly _localAccounts;
    private readonly _impersonatedAccounts;
    private _nextBlockTimestamp;
    private _userProvidedNextBlockBaseFeePerGas?;
    private _lastFilterId;
    private _filters;
    private _nextSnapshotId;
    private readonly _snapshots;
    private readonly _vmTracer;
    private readonly _vmTraceDecoder;
    private readonly _solidityTracer;
    private readonly _consoleLogger;
    private _failedStackTraces;
    private _irregularStatesByBlockNumber;
    private constructor();
    getSignedTransaction(txParams: TransactionParams): Promise<TypedTransaction>;
    sendTransaction(tx: TypedTransaction): Promise<SendTransactionResult>;
    mineBlock(timestamp?: bigint): Promise<MineBlockResult>;
    /**
     * Mines `count` blocks with a difference of `interval` seconds between their
     * timestamps.
     *
     * Returns an array with the results of the blocks that were really mined (the
     * ones that were reserved are not included).
     */
    mineBlocks(count?: bigint, interval?: bigint): Promise<MineBlockResult[]>;
    runCall(call: CallParams, blockNumberOrPending: bigint | "pending", stateOverrideSet?: StateOverrideSet): Promise<RunCallResult>;
    getAccountBalance(address: Address, blockNumberOrPending?: bigint | "pending"): Promise<bigint>;
    getNextConfirmedNonce(address: Address, blockNumberOrPending: bigint | "pending"): Promise<bigint>;
    getAccountNextPendingNonce(address: Address): Promise<bigint>;
    getCodeFromTrace(trace: MessageTrace | undefined, blockNumberOrPending: bigint | "pending"): Promise<Buffer>;
    getLatestBlock(): Promise<Block>;
    getLatestBlockNumber(): bigint;
    getPendingBlockAndTotalDifficulty(): Promise<[Block, bigint]>;
    getLocalAccountAddresses(): Promise<string[]>;
    getBlockGasLimit(): bigint;
    estimateGas(callParams: CallParams, blockNumberOrPending: bigint | "pending"): Promise<EstimateGasResult>;
    getGasPrice(): Promise<bigint>;
    getMaxPriorityFeePerGas(): Promise<bigint>;
    getCoinbaseAddress(): Address;
    getStorageAt(address: Address, positionIndex: bigint, blockNumberOrPending: bigint | "pending"): Promise<Buffer>;
    getBlockByNumber(pending: "pending"): Promise<Block>;
    getBlockByNumber(blockNumberOrPending: bigint | "pending"): Promise<Block | undefined>;
    getBlockByHash(blockHash: Buffer): Promise<Block | undefined>;
    getBlockByTransactionHash(hash: Buffer): Promise<Block | undefined>;
    getBlockTotalDifficulty(block: Block): Promise<bigint>;
    getCode(address: Address, blockNumberOrPending: bigint | "pending"): Promise<Buffer>;
    getNextBlockTimestamp(): bigint;
    setNextBlockTimestamp(timestamp: bigint): void;
    getTimeIncrement(): bigint;
    setTimeIncrement(timeIncrement: bigint): void;
    increaseTime(increment: bigint): void;
    setUserProvidedNextBlockBaseFeePerGas(baseFeePerGas: bigint): void;
    getUserProvidedNextBlockBaseFeePerGas(): bigint | undefined;
    private _resetUserProvidedNextBlockBaseFeePerGas;
    getNextBlockBaseFeePerGas(): Promise<bigint | undefined>;
    getPendingTransaction(hash: Buffer): Promise<TypedTransaction | undefined>;
    getTransactionReceipt(hash: Buffer | string): Promise<RpcReceiptOutput | undefined>;
    getPendingTransactions(): Promise<TypedTransaction[]>;
    signPersonalMessage(address: Address, data: Buffer): Promise<ECDSASignature>;
    signTypedDataV4(address: Address, typedData: any): Promise<string>;
    getStackTraceFailuresCount(): number;
    takeSnapshot(): Promise<number>;
    revertToSnapshot(id: number): Promise<boolean>;
    newFilter(filterParams: FilterParams, isSubscription: boolean): Promise<bigint>;
    newBlockFilter(isSubscription: boolean): Promise<bigint>;
    newPendingTransactionFilter(isSubscription: boolean): Promise<bigint>;
    uninstallFilter(filterId: bigint, subscription: boolean): Promise<boolean>;
    getFilterChanges(filterId: bigint): Promise<string[] | RpcLogOutput[] | undefined>;
    getFilterLogs(filterId: bigint): Promise<RpcLogOutput[] | undefined>;
    getLogs(filterParams: FilterParams): Promise<RpcLogOutput[]>;
    addCompilationResult(solcVersion: string, compilerInput: CompilerInput, compilerOutput: CompilerOutput): Promise<boolean>;
    addImpersonatedAccount(address: Buffer): true;
    removeImpersonatedAccount(address: Buffer): boolean;
    setAutomine(automine: boolean): void;
    getAutomine(): boolean;
    setBlockGasLimit(gasLimit: bigint | number): Promise<void>;
    setMinGasPrice(minGasPrice: bigint): Promise<void>;
    dropTransaction(hash: Buffer): Promise<boolean>;
    setAccountBalance(address: Address, newBalance: bigint): Promise<void>;
    setAccountCode(address: Address, newCode: Buffer): Promise<void>;
    setNextConfirmedNonce(address: Address, newNonce: bigint): Promise<void>;
    setStorageAt(address: Address, positionIndex: bigint, value: Buffer): Promise<void>;
    traceCall(callParams: CallParams, block: bigint | "pending", traceConfig: RpcDebugTracingConfig): Promise<import("./output").RpcDebugTraceOutput>;
    traceTransaction(hash: Buffer, config: RpcDebugTracingConfig): Promise<import("./output").RpcDebugTraceOutput>;
    getFeeHistory(blockCount: bigint, newestBlock: bigint | "pending", rewardPercentiles: number[]): Promise<FeeHistory>;
    setCoinbase(coinbase: Address): Promise<void>;
    private _getGasUsedRatio;
    private _getRewards;
    private _addPendingTransaction;
    private _mineTransaction;
    private _mineTransactionAndPending;
    private _mineBlocksUntilTransactionIsIncluded;
    private _gatherTraces;
    private _validateAutominedTx;
    /**
     * Mines a new block with as many pending txs as possible, adding it to
     * the VM's blockchain.
     *
     * This method reverts any modification to the state manager if it throws.
     */
    private _mineBlockWithPendingTxs;
    private _getMinimalTransactionFee;
    private _getFakeTransaction;
    private _getSnapshotIndex;
    private _removeSnapshot;
    private _initLocalAccounts;
    private _getConsoleLogMessages;
    private _manageErrors;
    private _calculateTimestampAndOffset;
    private _resetNextBlockTimestamp;
    private _notifyPendingTransaction;
    private _getLocalAccountPrivateKey;
    /**
     * Saves a block as successfully run. This method requires that the block
     * was added to the blockchain.
     */
    private _saveBlockAsSuccessfullyRun;
    private _timestampClashesWithPreviousBlockOne;
    private _runInBlockContext;
    private _runInPendingBlockContext;
    private _setBlockContext;
    private _restoreBlockContext;
    private _correctInitialEstimation;
    private _binarySearchEstimation;
    private _applyStateOverrideSet;
    private _overrideBalanceAndNonce;
    private _overrideCode;
    private _overrideStateAndStateDiff;
    /**
     * This function runs a transaction and reverts all the modifications that it
     * makes.
     */
    private _runTxAndRevertMutations;
    private _computeFilterParams;
    private _newDeadline;
    private _getNextFilterId;
    private _filterIdToFiltersKey;
    private _emitEthEvent;
    private _getNonce;
    private _isTransactionMined;
    private _isTxMinable;
    private _persistIrregularWorldState;
    isEip1559Active(blockNumberOrPending?: bigint | "pending"): boolean;
    isEip4895Active(blockNumberOrPending?: bigint | "pending"): boolean;
    isPostMergeHardfork(): boolean;
    setPrevRandao(prevRandao: Buffer): void;
    getClientVersion(): Promise<string>;
    getMetadata(): Promise<HardhatMetadata>;
    private _getNextMixHash;
    private _getEstimateGasFeePriceFields;
    private _selectHardfork;
    private _getCommonForTracing;
    private _getTransientStorageSettings;
}
//# sourceMappingURL=node.d.ts.map