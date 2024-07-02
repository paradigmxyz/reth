"use strict";
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? (function(o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
    o["default"] = v;
});
var __importStar = (this && this.__importStar) || function (mod) {
    if (mod && mod.__esModule) return mod;
    var result = {};
    if (mod != null) for (var k in mod) if (k !== "default" && Object.prototype.hasOwnProperty.call(mod, k)) __createBinding(result, mod, k);
    __setModuleDefault(result, mod);
    return result;
};
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.HardhatNode = void 0;
const ethereumjs_block_1 = require("@nomicfoundation/ethereumjs-block");
const ethereumjs_common_1 = require("@nomicfoundation/ethereumjs-common");
const ethereumjs_tx_1 = require("@nomicfoundation/ethereumjs-tx");
const ethereumjs_util_1 = require("@nomicfoundation/ethereumjs-util");
const ethereumjs_vm_1 = require("@nomicfoundation/ethereumjs-vm");
const ethereumjs_evm_1 = require("@nomicfoundation/ethereumjs-evm");
const exceptions_1 = require("@nomicfoundation/ethereumjs-evm/dist/exceptions");
const ethereumjs_statemanager_1 = require("@nomicfoundation/ethereumjs-statemanager");
const eth_sig_util_1 = require("@metamask/eth-sig-util");
const chalk_1 = __importDefault(require("chalk"));
const crypto_1 = require("crypto");
const debug_1 = __importDefault(require("debug"));
const events_1 = __importDefault(require("events"));
const BigIntUtils = __importStar(require("../../util/bigint"));
const constants_1 = require("../../constants");
const default_config_1 = require("../../core/config/default-config");
const errors_1 = require("../../core/errors");
const errors_2 = require("../../core/providers/errors");
const reporter_1 = require("../../sentry/reporter");
const date_1 = require("../../util/date");
const hardforks_1 = require("../../util/hardforks");
const packageInfo_1 = require("../../util/packageInfo");
const compiler_to_model_1 = require("../stack-traces/compiler-to-model");
const consoleLogger_1 = require("../stack-traces/consoleLogger");
const contracts_identifier_1 = require("../stack-traces/contracts-identifier");
const message_trace_1 = require("../stack-traces/message-trace");
const solidity_errors_1 = require("../stack-traces/solidity-errors");
const solidityTracer_1 = require("../stack-traces/solidityTracer");
const vm_debug_tracer_1 = require("../stack-traces/vm-debug-tracer");
const vm_trace_decoder_1 = require("../stack-traces/vm-trace-decoder");
const vm_tracer_1 = require("../stack-traces/vm-tracer");
require("./ethereumjs-workarounds");
const base_types_1 = require("../../core/jsonrpc/types/base-types");
const filter_1 = require("./filter");
const ForkBlockchain_1 = require("./fork/ForkBlockchain");
const ForkStateManager_1 = require("./fork/ForkStateManager");
const HardhatBlockchain_1 = require("./HardhatBlockchain");
const node_types_1 = require("./node-types");
const output_1 = require("./output");
const return_data_1 = require("./return-data");
const FakeSenderAccessListEIP2930Transaction_1 = require("./transactions/FakeSenderAccessListEIP2930Transaction");
const FakeSenderEIP1559Transaction_1 = require("./transactions/FakeSenderEIP1559Transaction");
const FakeSenderTransaction_1 = require("./transactions/FakeSenderTransaction");
const TxPool_1 = require("./TxPool");
const TransactionQueue_1 = require("./TransactionQueue");
const getCurrentTimestamp_1 = require("./utils/getCurrentTimestamp");
const makeCommon_1 = require("./utils/makeCommon");
const makeForkClient_1 = require("./utils/makeForkClient");
const makeStateTrie_1 = require("./utils/makeStateTrie");
const putGenesisBlock_1 = require("./utils/putGenesisBlock");
const txMapToArray_1 = require("./utils/txMapToArray");
const random_1 = require("./utils/random");
const log = (0, debug_1.default)("hardhat:core:hardhat-network:node");
/* eslint-disable @nomicfoundation/hardhat-internal-rules/only-hardhat-error */
class HardhatNode extends events_1.default {
    static async create(config) {
        const { automine, genesisAccounts, blockGasLimit, tracingConfig, minGasPrice, mempoolOrder, networkId, chainId, allowBlocksWithSameTimestamp, enableTransientStorage, } = config;
        const allowUnlimitedContractSize = config.allowUnlimitedContractSize ?? false;
        let stateManager;
        let blockchain;
        let initialBlockTimeOffset;
        let nextBlockBaseFeePerGas;
        let forkNetworkId;
        let forkBlockNum;
        let forkBlockHash;
        let hardforkActivations = new Map();
        const initialBaseFeePerGasConfig = config.initialBaseFeePerGas !== undefined
            ? BigInt(config.initialBaseFeePerGas)
            : undefined;
        const hardfork = (0, hardforks_1.getHardforkName)(config.hardfork);
        const mixHashGenerator = random_1.RandomBufferGenerator.create("randomMixHashSeed");
        let forkClient;
        const common = (0, makeCommon_1.makeCommon)(config);
        if ((0, node_types_1.isForkedNodeConfig)(config)) {
            const { forkClient: _forkClient, forkBlockNumber, forkBlockTimestamp, forkBlockHash: _forkBlockHash, } = await (0, makeForkClient_1.makeForkClient)(config.forkConfig, config.forkCachePath);
            forkClient = _forkClient;
            forkNetworkId = forkClient.getNetworkId();
            forkBlockNum = forkBlockNumber;
            forkBlockHash = _forkBlockHash;
            this._validateHardforks(config.forkConfig.blockNumber, common, forkNetworkId);
            const forkStateManager = new ForkStateManager_1.ForkStateManager(forkClient, forkBlockNumber);
            await forkStateManager.initializeGenesisAccounts(genesisAccounts);
            stateManager = forkStateManager;
            blockchain = new ForkBlockchain_1.ForkBlockchain(forkClient, forkBlockNumber, common);
            initialBlockTimeOffset = BigInt((0, date_1.getDifferenceInSeconds)(new Date(forkBlockTimestamp), new Date()));
            // If the hardfork is London or later we need a base fee per gas for the
            // first local block. If initialBaseFeePerGas config was provided we use
            // that. Otherwise, what we do depends on the block we forked from. If
            // it's an EIP-1559 block we don't need to do anything here, as we'll
            // end up automatically computing the next base fee per gas based on it.
            if ((0, hardforks_1.hardforkGte)(hardfork, hardforks_1.HardforkName.LONDON)) {
                if (initialBaseFeePerGasConfig !== undefined) {
                    nextBlockBaseFeePerGas = initialBaseFeePerGasConfig;
                }
                else {
                    const latestBlock = await blockchain.getLatestBlock();
                    if (latestBlock.header.baseFeePerGas === undefined) {
                        nextBlockBaseFeePerGas = BigInt(default_config_1.HARDHAT_NETWORK_DEFAULT_INITIAL_BASE_FEE_PER_GAS);
                    }
                }
            }
            if (config.chains.has(forkNetworkId)) {
                hardforkActivations = config.chains.get(forkNetworkId).hardforkHistory;
            }
        }
        else {
            const stateTrie = await (0, makeStateTrie_1.makeStateTrie)(genesisAccounts);
            stateManager = new ethereumjs_statemanager_1.DefaultStateManager({
                trie: stateTrie,
            });
            const hardhatBlockchain = new HardhatBlockchain_1.HardhatBlockchain(common);
            const genesisBlockBaseFeePerGas = (0, hardforks_1.hardforkGte)(hardfork, hardforks_1.HardforkName.LONDON)
                ? initialBaseFeePerGasConfig ??
                    BigInt(default_config_1.HARDHAT_NETWORK_DEFAULT_INITIAL_BASE_FEE_PER_GAS)
                : undefined;
            await (0, putGenesisBlock_1.putGenesisBlock)(hardhatBlockchain, common, config, stateTrie, hardfork, mixHashGenerator.next(), genesisBlockBaseFeePerGas);
            if (config.initialDate !== undefined) {
                initialBlockTimeOffset = BigInt((0, date_1.getDifferenceInSeconds)(config.initialDate, new Date()));
            }
            blockchain = hardhatBlockchain;
        }
        const txPool = new TxPool_1.TxPool(stateManager, BigInt(blockGasLimit), common);
        const eei = new ethereumjs_vm_1.EEI(stateManager, common, blockchain);
        const evm = await ethereumjs_evm_1.EVM.create({
            eei,
            allowUnlimitedContractSize,
            common,
        });
        const vm = await ethereumjs_vm_1.VM.create({
            evm,
            activatePrecompiles: true,
            common,
            stateManager,
            blockchain,
        });
        const instanceId = (0, ethereumjs_util_1.bufferToBigInt)((0, crypto_1.randomBytes)(32));
        const node = new HardhatNode(vm, instanceId, stateManager, blockchain, txPool, automine, minGasPrice, initialBlockTimeOffset, mempoolOrder, config.coinbase, genesisAccounts, networkId, chainId, hardfork, hardforkActivations, mixHashGenerator, allowUnlimitedContractSize, allowBlocksWithSameTimestamp, tracingConfig, forkNetworkId, forkBlockNum, forkBlockHash, nextBlockBaseFeePerGas, forkClient, enableTransientStorage);
        return [common, node];
    }
    static _validateHardforks(forkBlockNumber, common, remoteChainId) {
        if (!common.gteHardfork("spuriousDragon")) {
            throw new errors_2.InternalError(`Invalid hardfork selected in Hardhat Network's config.

The hardfork must be at least spuriousDragon, but ${common.hardfork()} was given.`);
        }
        if (forkBlockNumber !== undefined) {
            let upstreamCommon;
            try {
                upstreamCommon = new ethereumjs_common_1.Common({ chain: remoteChainId });
            }
            catch {
                // If ethereumjs doesn't have a common it will throw and we won't have
                // info about the activation block of each hardfork, so we don't run
                // this validation.
                return;
            }
            upstreamCommon.setHardforkByBlockNumber(forkBlockNumber);
            if (!upstreamCommon.gteHardfork("spuriousDragon")) {
                throw new errors_2.InternalError(`Cannot fork ${upstreamCommon.chainName()} from block ${forkBlockNumber}.

Hardhat Network's forking functionality only works with blocks from at least spuriousDragon.`);
            }
        }
    }
    constructor(_vm, _instanceId, _stateManager, _blockchain, _txPool, _automine, _minGasPrice, _blockTimeOffsetSeconds = 0n, _mempoolOrder, _coinbase, genesisAccounts, _configNetworkId, _configChainId, hardfork, _hardforkActivations, _mixHashGenerator, allowUnlimitedContractSize, _allowBlocksWithSameTimestamp, tracingConfig, _forkNetworkId, _forkBlockNumber, _forkBlockHash, nextBlockBaseFee, _forkClient, _enableTransientStorage = false) {
        super();
        this._vm = _vm;
        this._instanceId = _instanceId;
        this._stateManager = _stateManager;
        this._blockchain = _blockchain;
        this._txPool = _txPool;
        this._automine = _automine;
        this._minGasPrice = _minGasPrice;
        this._blockTimeOffsetSeconds = _blockTimeOffsetSeconds;
        this._mempoolOrder = _mempoolOrder;
        this._coinbase = _coinbase;
        this._configNetworkId = _configNetworkId;
        this._configChainId = _configChainId;
        this.hardfork = hardfork;
        this._hardforkActivations = _hardforkActivations;
        this._mixHashGenerator = _mixHashGenerator;
        this.allowUnlimitedContractSize = allowUnlimitedContractSize;
        this._allowBlocksWithSameTimestamp = _allowBlocksWithSameTimestamp;
        this._forkNetworkId = _forkNetworkId;
        this._forkBlockNumber = _forkBlockNumber;
        this._forkBlockHash = _forkBlockHash;
        this._forkClient = _forkClient;
        this._enableTransientStorage = _enableTransientStorage;
        this._localAccounts = new Map(); // address => private key
        this._impersonatedAccounts = new Set(); // address
        this._nextBlockTimestamp = 0n;
        this._lastFilterId = 0n;
        this._filters = new Map();
        this._nextSnapshotId = 1; // We start in 1 to mimic Ganache
        this._snapshots = [];
        this._consoleLogger = new consoleLogger_1.ConsoleLogger();
        this._failedStackTraces = 0;
        // blockNumber => state root
        this._irregularStatesByBlockNumber = new Map();
        this._initLocalAccounts(genesisAccounts);
        if (nextBlockBaseFee !== undefined) {
            this.setUserProvidedNextBlockBaseFeePerGas(nextBlockBaseFee);
        }
        this._vmTracer = new vm_tracer_1.VMTracer(this._vm, this._stateManager.getContractCode.bind(this._stateManager), false);
        this._vmTracer.enableTracing();
        const contractsIdentifier = new contracts_identifier_1.ContractsIdentifier();
        this._vmTraceDecoder = new vm_trace_decoder_1.VmTraceDecoder(contractsIdentifier);
        this._solidityTracer = new solidityTracer_1.SolidityTracer();
        if (tracingConfig === undefined || tracingConfig.buildInfos === undefined) {
            return;
        }
        try {
            for (const buildInfo of tracingConfig.buildInfos) {
                const bytecodes = (0, compiler_to_model_1.createModelsAndDecodeBytecodes)(buildInfo.solcVersion, buildInfo.input, buildInfo.output);
                for (const bytecode of bytecodes) {
                    this._vmTraceDecoder.addBytecode(bytecode);
                }
            }
        }
        catch (error) {
            console.warn(chalk_1.default.yellow("The Hardhat Network tracing engine could not be initialized. Run Hardhat with --verbose to learn more."));
            log("Hardhat Network tracing disabled: ContractsIdentifier failed to be initialized. Please report this to help us improve Hardhat.\n", error);
            if (error instanceof Error) {
                reporter_1.Reporter.reportError(error);
            }
        }
    }
    async getSignedTransaction(txParams) {
        const senderAddress = (0, ethereumjs_util_1.bufferToHex)(txParams.from);
        const pk = this._localAccounts.get(senderAddress);
        if (pk !== undefined) {
            let tx;
            if ("maxFeePerGas" in txParams) {
                tx = ethereumjs_tx_1.FeeMarketEIP1559Transaction.fromTxData(txParams, {
                    common: this._vm._common,
                    disableMaxInitCodeSizeCheck: true,
                });
            }
            else if ("accessList" in txParams) {
                tx = ethereumjs_tx_1.AccessListEIP2930Transaction.fromTxData(txParams, {
                    common: this._vm._common,
                    disableMaxInitCodeSizeCheck: true,
                });
            }
            else {
                tx = ethereumjs_tx_1.Transaction.fromTxData(txParams, {
                    common: this._vm._common,
                    disableMaxInitCodeSizeCheck: true,
                });
            }
            return tx.sign(pk);
        }
        if (this._impersonatedAccounts.has(senderAddress)) {
            return this._getFakeTransaction(txParams);
        }
        throw new errors_2.InvalidInputError(`unknown account ${senderAddress}`);
    }
    async sendTransaction(tx) {
        if (!this._automine) {
            return this._addPendingTransaction(tx);
        }
        await this._validateAutominedTx(tx);
        if (this._txPool.hasPendingTransactions() ||
            this._txPool.hasQueuedTransactions()) {
            return this._mineTransactionAndPending(tx);
        }
        return this._mineTransaction(tx);
    }
    async mineBlock(timestamp) {
        const timestampAndOffset = this._calculateTimestampAndOffset(timestamp);
        let [blockTimestamp] = timestampAndOffset;
        const [, offsetShouldChange, newOffset] = timestampAndOffset;
        const needsTimestampIncrease = !this._allowBlocksWithSameTimestamp &&
            (await this._timestampClashesWithPreviousBlockOne(blockTimestamp));
        if (needsTimestampIncrease) {
            blockTimestamp += 1n;
        }
        let result;
        try {
            result = await this._mineBlockWithPendingTxs(blockTimestamp);
        }
        catch (err) {
            if (err instanceof Error) {
                if (err?.message.includes("sender doesn't have enough funds")) {
                    throw new errors_2.InvalidInputError(err.message, err);
                }
                // Some network errors are HardhatErrors, and can end up here when forking
                if (errors_1.HardhatError.isHardhatError(err)) {
                    throw err;
                }
                throw new errors_2.TransactionExecutionError(err);
            }
            // eslint-disable-next-line @nomicfoundation/hardhat-internal-rules/only-hardhat-error
            throw err;
        }
        await this._saveBlockAsSuccessfullyRun(result.block, result.blockResult);
        if (needsTimestampIncrease) {
            this.increaseTime(1n);
        }
        if (offsetShouldChange) {
            this.setTimeIncrement(newOffset);
        }
        this._resetNextBlockTimestamp();
        this._resetUserProvidedNextBlockBaseFeePerGas();
        return result;
    }
    /**
     * Mines `count` blocks with a difference of `interval` seconds between their
     * timestamps.
     *
     * Returns an array with the results of the blocks that were really mined (the
     * ones that were reserved are not included).
     */
    async mineBlocks(count = 1n, interval = 1n) {
        if (count === 0n) {
            // nothing to do
            return [];
        }
        const mineBlockResults = [];
        // we always mine the first block, and we don't apply the interval for it
        mineBlockResults.push(await this.mineBlock());
        // helper function to mine a block with a timstamp that respects the
        // interval
        const mineBlock = async () => {
            const nextTimestamp = (await this.getLatestBlock()).header.timestamp + interval;
            mineBlockResults.push(await this.mineBlock(nextTimestamp));
        };
        // then we mine any pending transactions
        while (count > mineBlockResults.length &&
            this._txPool.hasPendingTransactions()) {
            await mineBlock();
        }
        // If there is at least one remaining block, we mine one. This way, we
        // guarantee that there's an empty block immediately before and after the
        // reservation. This makes the logging easier to get right.
        if (count > mineBlockResults.length) {
            await mineBlock();
        }
        const remainingBlockCount = count - BigInt(mineBlockResults.length);
        // There should be at least 2 blocks left for the reservation to work,
        // because we always mine a block after it. But here we use a bigger
        // number to err on the safer side.
        if (remainingBlockCount <= 5) {
            // if there are few blocks left to mine, we just mine them
            while (count > mineBlockResults.length) {
                await mineBlock();
            }
            return mineBlockResults;
        }
        // otherwise, we reserve a range and mine the last one
        const latestBlock = await this.getLatestBlock();
        this._blockchain.reserveBlocks(remainingBlockCount - 1n, interval, await this._stateManager.getStateRoot(), await this.getBlockTotalDifficulty(latestBlock), (await this.getLatestBlock()).header.baseFeePerGas);
        await mineBlock();
        return mineBlockResults;
    }
    async runCall(call, blockNumberOrPending, stateOverrideSet = {}) {
        let txParams;
        const nonce = await this._getNonce(new ethereumjs_util_1.Address(call.from), blockNumberOrPending);
        if (call.gasPrice !== undefined ||
            !this.isEip1559Active(blockNumberOrPending)) {
            txParams = {
                gasPrice: 0n,
                nonce,
                ...call,
            };
        }
        else {
            const maxFeePerGas = call.maxFeePerGas ?? call.maxPriorityFeePerGas ?? 0n;
            const maxPriorityFeePerGas = call.maxPriorityFeePerGas ?? 0n;
            txParams = {
                ...call,
                nonce,
                maxFeePerGas,
                maxPriorityFeePerGas,
                accessList: call.accessList ?? [],
            };
        }
        const tx = await this._getFakeTransaction(txParams);
        const result = await this._runInBlockContext(blockNumberOrPending, async () => this._runTxAndRevertMutations(tx, blockNumberOrPending, true, stateOverrideSet));
        const traces = await this._gatherTraces(result.execResult);
        return {
            ...traces,
            result: new return_data_1.ReturnData(result.execResult.returnValue),
        };
    }
    async getAccountBalance(address, blockNumberOrPending) {
        if (blockNumberOrPending === undefined) {
            blockNumberOrPending = this.getLatestBlockNumber();
        }
        const account = await this._runInBlockContext(blockNumberOrPending, () => this._stateManager.getAccount(address));
        return account.balance;
    }
    async getNextConfirmedNonce(address, blockNumberOrPending) {
        const account = await this._runInBlockContext(blockNumberOrPending, () => this._stateManager.getAccount(address));
        return account.nonce;
    }
    async getAccountNextPendingNonce(address) {
        return this._txPool.getNextPendingNonce(address);
    }
    async getCodeFromTrace(trace, blockNumberOrPending) {
        if (trace === undefined ||
            (0, message_trace_1.isPrecompileTrace)(trace) ||
            (0, message_trace_1.isCreateTrace)(trace)) {
            return Buffer.from("");
        }
        return this.getCode(new ethereumjs_util_1.Address(trace.address), blockNumberOrPending);
    }
    async getLatestBlock() {
        return this._blockchain.getLatestBlock();
    }
    getLatestBlockNumber() {
        return this._blockchain.getLatestBlockNumber();
    }
    async getPendingBlockAndTotalDifficulty() {
        return this._runInPendingBlockContext(async () => {
            const block = await this._blockchain.getLatestBlock();
            const totalDifficulty = await this._blockchain.getTotalDifficulty(block.hash());
            return [block, totalDifficulty];
        });
    }
    async getLocalAccountAddresses() {
        return [...this._localAccounts.keys()];
    }
    getBlockGasLimit() {
        return this._txPool.getBlockGasLimit();
    }
    async estimateGas(callParams, blockNumberOrPending) {
        // We get the CallParams and transform it into a TransactionParams to be
        // able to run it.
        const nonce = await this._getNonce(new ethereumjs_util_1.Address(callParams.from), blockNumberOrPending);
        // TODO: This is more complex in Geth, we should make sure we aren't missing
        //  anything here.
        const feePriceFields = await this._getEstimateGasFeePriceFields(callParams, blockNumberOrPending);
        let txParams;
        if ("gasPrice" in feePriceFields) {
            if (callParams.accessList === undefined) {
                // Legacy tx
                txParams = {
                    ...callParams,
                    nonce,
                    gasPrice: feePriceFields.gasPrice,
                };
            }
            else {
                // Access list tx
                txParams = {
                    ...callParams,
                    nonce,
                    gasPrice: feePriceFields.gasPrice,
                    accessList: callParams.accessList ?? [],
                };
            }
        }
        else {
            // EIP-1559 tx
            txParams = {
                ...callParams,
                nonce,
                maxFeePerGas: feePriceFields.maxFeePerGas,
                maxPriorityFeePerGas: feePriceFields.maxPriorityFeePerGas,
                accessList: callParams.accessList ?? [],
            };
        }
        const tx = await this._getFakeTransaction(txParams);
        // TODO: This may not work if there are multiple txs in the mempool and
        //  the one being estimated won't fit in the first block, or maybe even
        //  if the state accessed by the tx changes after it is executed within
        //  the first block.
        const result = await this._runInBlockContext(blockNumberOrPending, () => this._runTxAndRevertMutations(tx, blockNumberOrPending));
        let vmTrace = this._vmTracer.getLastTopLevelMessageTrace();
        const vmTracerError = this._vmTracer.getLastError();
        this._vmTracer.clearLastError();
        if (vmTrace !== undefined) {
            vmTrace = this._vmTraceDecoder.tryToDecodeMessageTrace(vmTrace);
        }
        const consoleLogMessages = await this._getConsoleLogMessages(vmTrace, vmTracerError);
        // This is only considered if the call to _runTxAndRevertMutations doesn't
        // manage errors
        if (result.execResult.exceptionError !== undefined) {
            return {
                estimation: this.getBlockGasLimit(),
                trace: vmTrace,
                error: await this._manageErrors(result.execResult, vmTrace, vmTracerError),
                consoleLogMessages,
            };
        }
        const initialEstimation = result.totalGasSpent;
        return {
            estimation: await this._correctInitialEstimation(blockNumberOrPending, txParams, initialEstimation),
            trace: vmTrace,
            consoleLogMessages,
        };
    }
    async getGasPrice() {
        const nextBlockBaseFeePerGas = await this.getNextBlockBaseFeePerGas();
        if (nextBlockBaseFeePerGas === undefined) {
            // We return a hardcoded value for networks without EIP-1559
            return 8n * 10n ** 9n;
        }
        const suggestedPriorityFeePerGas = 10n ** 9n;
        return nextBlockBaseFeePerGas + suggestedPriorityFeePerGas;
    }
    async getMaxPriorityFeePerGas() {
        return BigInt(default_config_1.HARDHAT_NETWORK_DEFAULT_MAX_PRIORITY_FEE_PER_GAS);
    }
    getCoinbaseAddress() {
        return ethereumjs_util_1.Address.fromString(this._coinbase);
    }
    async getStorageAt(address, positionIndex, blockNumberOrPending) {
        const key = (0, ethereumjs_util_1.setLengthLeft)((0, ethereumjs_util_1.bigIntToBuffer)(positionIndex), 32);
        const data = await this._runInBlockContext(blockNumberOrPending, () => this._stateManager.getContractStorage(address, key));
        const EXPECTED_DATA_SIZE = 32;
        if (data.length < EXPECTED_DATA_SIZE) {
            return Buffer.concat([Buffer.alloc(EXPECTED_DATA_SIZE - data.length, 0), data], EXPECTED_DATA_SIZE);
        }
        return data;
    }
    async getBlockByNumber(blockNumberOrPending) {
        if (blockNumberOrPending === "pending") {
            return this._runInPendingBlockContext(() => this._blockchain.getLatestBlock());
        }
        try {
            const block = await this._blockchain.getBlock(blockNumberOrPending);
            return block;
        }
        catch {
            return undefined;
        }
    }
    async getBlockByHash(blockHash) {
        try {
            const block = await this._blockchain.getBlock(blockHash);
            return block;
        }
        catch {
            return undefined;
        }
    }
    async getBlockByTransactionHash(hash) {
        const block = await this._blockchain.getBlockByTransactionHash(hash);
        return block ?? undefined;
    }
    async getBlockTotalDifficulty(block) {
        return this._blockchain.getTotalDifficulty(block.hash());
    }
    async getCode(address, blockNumberOrPending) {
        return this._runInBlockContext(blockNumberOrPending, () => this._stateManager.getContractCode(address));
    }
    getNextBlockTimestamp() {
        return this._nextBlockTimestamp;
    }
    setNextBlockTimestamp(timestamp) {
        this._nextBlockTimestamp = timestamp;
    }
    getTimeIncrement() {
        return this._blockTimeOffsetSeconds;
    }
    setTimeIncrement(timeIncrement) {
        this._blockTimeOffsetSeconds = timeIncrement;
    }
    increaseTime(increment) {
        this._blockTimeOffsetSeconds += increment;
    }
    setUserProvidedNextBlockBaseFeePerGas(baseFeePerGas) {
        this._userProvidedNextBlockBaseFeePerGas = baseFeePerGas;
    }
    getUserProvidedNextBlockBaseFeePerGas() {
        return this._userProvidedNextBlockBaseFeePerGas;
    }
    _resetUserProvidedNextBlockBaseFeePerGas() {
        this._userProvidedNextBlockBaseFeePerGas = undefined;
    }
    async getNextBlockBaseFeePerGas() {
        if (!this.isEip1559Active()) {
            return undefined;
        }
        const userDefined = this.getUserProvidedNextBlockBaseFeePerGas();
        if (userDefined !== undefined) {
            return userDefined;
        }
        const latestBlock = await this.getLatestBlock();
        return latestBlock.header.calcNextBaseFee();
    }
    async getPendingTransaction(hash) {
        return this._txPool.getTransactionByHash(hash)?.data;
    }
    async getTransactionReceipt(hash) {
        const hashBuffer = hash instanceof Buffer ? hash : (0, ethereumjs_util_1.toBuffer)(hash);
        const receipt = await this._blockchain.getTransactionReceipt(hashBuffer);
        return receipt ?? undefined;
    }
    async getPendingTransactions() {
        const txPoolPending = (0, txMapToArray_1.txMapToArray)(this._txPool.getPendingTransactions());
        const txPoolQueued = (0, txMapToArray_1.txMapToArray)(this._txPool.getQueuedTransactions());
        return txPoolPending.concat(txPoolQueued);
    }
    async signPersonalMessage(address, data) {
        const messageHash = (0, ethereumjs_util_1.hashPersonalMessage)(data);
        const privateKey = this._getLocalAccountPrivateKey(address);
        return (0, ethereumjs_util_1.ecsign)(messageHash, privateKey);
    }
    async signTypedDataV4(address, typedData) {
        const privateKey = this._getLocalAccountPrivateKey(address);
        return (0, eth_sig_util_1.signTypedData)({
            privateKey,
            version: eth_sig_util_1.SignTypedDataVersion.V4,
            data: typedData,
        });
    }
    getStackTraceFailuresCount() {
        return this._failedStackTraces;
    }
    async takeSnapshot() {
        const id = this._nextSnapshotId;
        const snapshot = {
            id,
            date: new Date(),
            latestBlock: await this.getLatestBlock(),
            stateRoot: await this._stateManager.getStateRoot(),
            txPoolSnapshotId: this._txPool.snapshot(),
            blockTimeOffsetSeconds: this.getTimeIncrement(),
            nextBlockTimestamp: this.getNextBlockTimestamp(),
            irregularStatesByBlockNumber: this._irregularStatesByBlockNumber,
            userProvidedNextBlockBaseFeePerGas: this.getUserProvidedNextBlockBaseFeePerGas(),
            coinbase: this.getCoinbaseAddress().toString(),
            mixHashGenerator: this._mixHashGenerator.clone(),
        };
        this._irregularStatesByBlockNumber = new Map(this._irregularStatesByBlockNumber);
        this._snapshots.push(snapshot);
        this._nextSnapshotId += 1;
        return id;
    }
    async revertToSnapshot(id) {
        const snapshotIndex = this._getSnapshotIndex(id);
        if (snapshotIndex === undefined) {
            return false;
        }
        const snapshot = this._snapshots[snapshotIndex];
        // We compute a new offset such that
        //  now + new_offset === snapshot_date + old_offset
        const now = new Date();
        const offsetToSnapshotInMillis = snapshot.date.valueOf() - now.valueOf();
        const offsetToSnapshotInSecs = Math.ceil(offsetToSnapshotInMillis / 1000);
        const newOffset = snapshot.blockTimeOffsetSeconds + BigInt(offsetToSnapshotInSecs);
        // We delete all following blocks, changes the state root, and all the
        // relevant Node fields.
        //
        // Note: There's no need to copy the maps here, as snapshots can only be
        // used once
        this._blockchain.deleteLaterBlocks(snapshot.latestBlock);
        this._irregularStatesByBlockNumber = snapshot.irregularStatesByBlockNumber;
        const irregularStateOrUndefined = this._irregularStatesByBlockNumber.get((await this.getLatestBlock()).header.number);
        await this._stateManager.setStateRoot(irregularStateOrUndefined ?? snapshot.stateRoot);
        this.setTimeIncrement(newOffset);
        this.setNextBlockTimestamp(snapshot.nextBlockTimestamp);
        this._txPool.revert(snapshot.txPoolSnapshotId);
        if (snapshot.userProvidedNextBlockBaseFeePerGas !== undefined) {
            this.setUserProvidedNextBlockBaseFeePerGas(snapshot.userProvidedNextBlockBaseFeePerGas);
        }
        else {
            this._resetUserProvidedNextBlockBaseFeePerGas();
        }
        this._coinbase = snapshot.coinbase;
        this._mixHashGenerator = snapshot.mixHashGenerator;
        // We delete this and the following snapshots, as they can only be used
        // once in Ganache
        this._snapshots.splice(snapshotIndex);
        return true;
    }
    async newFilter(filterParams, isSubscription) {
        filterParams = await this._computeFilterParams(filterParams, true);
        const filterId = this._getNextFilterId();
        this._filters.set(this._filterIdToFiltersKey(filterId), {
            id: filterId,
            type: filter_1.Type.LOGS_SUBSCRIPTION,
            criteria: {
                fromBlock: filterParams.fromBlock,
                toBlock: filterParams.toBlock,
                addresses: filterParams.addresses,
                normalizedTopics: filterParams.normalizedTopics,
            },
            deadline: this._newDeadline(),
            hashes: [],
            logs: await this.getLogs(filterParams),
            subscription: isSubscription,
        });
        return filterId;
    }
    async newBlockFilter(isSubscription) {
        const block = await this.getLatestBlock();
        const filterId = this._getNextFilterId();
        this._filters.set(this._filterIdToFiltersKey(filterId), {
            id: filterId,
            type: filter_1.Type.BLOCK_SUBSCRIPTION,
            deadline: this._newDeadline(),
            hashes: [(0, ethereumjs_util_1.bufferToHex)(block.header.hash())],
            logs: [],
            subscription: isSubscription,
        });
        return filterId;
    }
    async newPendingTransactionFilter(isSubscription) {
        const filterId = this._getNextFilterId();
        this._filters.set(this._filterIdToFiltersKey(filterId), {
            id: filterId,
            type: filter_1.Type.PENDING_TRANSACTION_SUBSCRIPTION,
            deadline: this._newDeadline(),
            hashes: [],
            logs: [],
            subscription: isSubscription,
        });
        return filterId;
    }
    async uninstallFilter(filterId, subscription) {
        const key = this._filterIdToFiltersKey(filterId);
        const filter = this._filters.get(key);
        if (filter === undefined) {
            return false;
        }
        if ((filter.subscription && !subscription) ||
            (!filter.subscription && subscription)) {
            return false;
        }
        this._filters.delete(key);
        return true;
    }
    async getFilterChanges(filterId) {
        const key = this._filterIdToFiltersKey(filterId);
        const filter = this._filters.get(key);
        if (filter === undefined) {
            return undefined;
        }
        filter.deadline = this._newDeadline();
        switch (filter.type) {
            case filter_1.Type.BLOCK_SUBSCRIPTION:
            case filter_1.Type.PENDING_TRANSACTION_SUBSCRIPTION:
                const hashes = filter.hashes;
                filter.hashes = [];
                return hashes;
            case filter_1.Type.LOGS_SUBSCRIPTION:
                const logs = filter.logs;
                filter.logs = [];
                return logs;
        }
        return undefined;
    }
    async getFilterLogs(filterId) {
        const key = this._filterIdToFiltersKey(filterId);
        const filter = this._filters.get(key);
        if (filter === undefined) {
            return undefined;
        }
        const logs = filter.logs;
        filter.logs = [];
        filter.deadline = this._newDeadline();
        return logs;
    }
    async getLogs(filterParams) {
        filterParams = await this._computeFilterParams(filterParams, false);
        return this._blockchain.getLogs(filterParams);
    }
    async addCompilationResult(solcVersion, compilerInput, compilerOutput) {
        let bytecodes;
        try {
            bytecodes = (0, compiler_to_model_1.createModelsAndDecodeBytecodes)(solcVersion, compilerInput, compilerOutput);
        }
        catch (error) {
            console.warn(chalk_1.default.yellow("The Hardhat Network tracing engine could not be updated. Run Hardhat with --verbose to learn more."));
            log("ContractsIdentifier failed to be updated. Please report this to help us improve Hardhat.\n", error);
            return false;
        }
        for (const bytecode of bytecodes) {
            this._vmTraceDecoder.addBytecode(bytecode);
        }
        return true;
    }
    addImpersonatedAccount(address) {
        this._impersonatedAccounts.add((0, ethereumjs_util_1.bufferToHex)(address));
        return true;
    }
    removeImpersonatedAccount(address) {
        return this._impersonatedAccounts.delete((0, ethereumjs_util_1.bufferToHex)(address));
    }
    setAutomine(automine) {
        this._automine = automine;
    }
    getAutomine() {
        return this._automine;
    }
    async setBlockGasLimit(gasLimit) {
        this._txPool.setBlockGasLimit(gasLimit);
        await this._txPool.updatePendingAndQueued();
    }
    async setMinGasPrice(minGasPrice) {
        this._minGasPrice = minGasPrice;
    }
    async dropTransaction(hash) {
        const removed = this._txPool.removeTransaction(hash);
        if (removed) {
            return true;
        }
        const isTransactionMined = await this._isTransactionMined(hash);
        if (isTransactionMined) {
            throw new errors_2.InvalidArgumentsError(`Transaction ${(0, ethereumjs_util_1.bufferToHex)(hash)} cannot be dropped because it's already mined`);
        }
        return false;
    }
    async setAccountBalance(address, newBalance) {
        const account = await this._stateManager.getAccount(address);
        account.balance = newBalance;
        await this._stateManager.putAccount(address, account);
        await this._persistIrregularWorldState();
    }
    async setAccountCode(address, newCode) {
        await this._stateManager.putContractCode(address, newCode);
        await this._persistIrregularWorldState();
    }
    async setNextConfirmedNonce(address, newNonce) {
        if (!this._txPool.isEmpty()) {
            throw new errors_2.InternalError("Cannot set account nonce when the transaction pool is not empty");
        }
        const account = await this._stateManager.getAccount(address);
        if (newNonce < account.nonce) {
            throw new errors_2.InvalidInputError(`New nonce (${newNonce.toString()}) must not be smaller than the existing nonce (${account.nonce.toString()})`);
        }
        account.nonce = newNonce;
        await this._stateManager.putAccount(address, account);
        await this._persistIrregularWorldState();
    }
    async setStorageAt(address, positionIndex, value) {
        await this._stateManager.putContractStorage(address, (0, ethereumjs_util_1.setLengthLeft)((0, ethereumjs_util_1.bigIntToBuffer)(positionIndex), 32), value);
        await this._persistIrregularWorldState();
    }
    async traceCall(callParams, block, traceConfig) {
        const vmDebugTracer = new vm_debug_tracer_1.VMDebugTracer(this._vm);
        return vmDebugTracer.trace(async () => {
            await this.runCall(callParams, block);
        }, traceConfig);
    }
    async traceTransaction(hash, config) {
        const block = await this.getBlockByTransactionHash(hash);
        if (block === undefined) {
            throw new errors_2.InvalidInputError(`Unable to find a block containing transaction ${(0, ethereumjs_util_1.bufferToHex)(hash)}`);
        }
        return this._runInBlockContext(block.header.number - 1n, async () => {
            const blockNumber = block.header.number;
            const blockchain = this._blockchain;
            let vm = this._vm;
            if (blockchain instanceof ForkBlockchain_1.ForkBlockchain &&
                blockNumber <= blockchain.getForkBlockNumber()) {
                (0, errors_1.assertHardhatInvariant)(this._forkNetworkId !== undefined, "this._forkNetworkId should exist if the blockchain is an instance of ForkBlockchain");
                const common = this._getCommonForTracing(this._forkNetworkId, blockNumber);
                vm = await ethereumjs_vm_1.VM.create({
                    common,
                    activatePrecompiles: true,
                    stateManager: this._vm.stateManager,
                    blockchain: this._vm.blockchain,
                });
            }
            // We don't support tracing transactions before the spuriousDragon fork
            // to avoid having to distinguish between empty and non-existing accounts.
            // We *could* do it during the non-forked mode, but for simplicity we just
            // don't support it at all.
            const isPreSpuriousDragon = !vm._common.gteHardfork("spuriousDragon");
            if (isPreSpuriousDragon) {
                throw new errors_2.InvalidInputError("Tracing is not supported for transactions using hardforks older than Spurious Dragon. ");
            }
            for (const tx of block.transactions) {
                let txWithCommon;
                const sender = tx.getSenderAddress();
                if (tx.type === 0) {
                    txWithCommon = new FakeSenderTransaction_1.FakeSenderTransaction(sender, tx, {
                        common: vm._common,
                    });
                }
                else if (tx.type === 1) {
                    txWithCommon = new FakeSenderAccessListEIP2930Transaction_1.FakeSenderAccessListEIP2930Transaction(sender, tx, { common: vm._common });
                }
                else if (tx.type === 2) {
                    txWithCommon = new FakeSenderEIP1559Transaction_1.FakeSenderEIP1559Transaction(sender, { ...tx, gasPrice: undefined }, { common: vm._common });
                }
                else {
                    throw new errors_2.InternalError("Only legacy, EIP2930, and EIP1559 txs are supported");
                }
                const txHash = txWithCommon.hash();
                if (txHash.equals(hash)) {
                    const vmDebugTracer = new vm_debug_tracer_1.VMDebugTracer(vm);
                    return vmDebugTracer.trace(async () => {
                        await vm.runTx({
                            tx: txWithCommon,
                            block,
                            skipHardForkValidation: true,
                        });
                    }, config);
                }
                await vm.runTx({
                    tx: txWithCommon,
                    block,
                    skipHardForkValidation: true,
                });
            }
            throw new errors_2.TransactionExecutionError(`Unable to find a transaction in a block that contains that transaction, this should never happen`);
        });
    }
    async getFeeHistory(blockCount, newestBlock, rewardPercentiles) {
        const latestBlock = this.getLatestBlockNumber();
        const pendingBlockNumber = latestBlock + 1n;
        const resolvedNewestBlock = newestBlock === "pending" ? pendingBlockNumber : newestBlock;
        const oldestBlock = BigIntUtils.max(resolvedNewestBlock - blockCount + 1n, 0n);
        // This is part of a temporary fix to https://github.com/NomicFoundation/hardhat/issues/2380
        const rangeIncludesRemoteBlocks = this._forkBlockNumber !== undefined &&
            oldestBlock <= this._forkBlockNumber;
        const baseFeePerGas = [];
        const gasUsedRatio = [];
        const reward = [];
        const lastBlock = resolvedNewestBlock + 1n;
        // This is part of a temporary fix to https://github.com/NomicFoundation/hardhat/issues/2380
        if (rangeIncludesRemoteBlocks) {
            try {
                const lastRemoteBlock = BigIntUtils.min(BigInt(this._forkBlockNumber), lastBlock);
                const remoteBlockCount = lastRemoteBlock - oldestBlock + 1n;
                const remoteValues = await this._forkClient.getFeeHistory(remoteBlockCount, lastRemoteBlock, rewardPercentiles);
                baseFeePerGas.push(...remoteValues.baseFeePerGas);
                gasUsedRatio.push(...remoteValues.gasUsedRatio);
                if (remoteValues.reward !== undefined) {
                    reward.push(...remoteValues.reward);
                }
            }
            catch (e) {
                // TODO: we can return less blocks here still be compliant with the spec
                throw new errors_2.InternalError("Remote node did not answer to eth_feeHistory correctly", e instanceof Error ? e : undefined);
            }
        }
        // We get the pending block here, and only if necessary, as it's something
        // costly to do.
        let pendingBlock;
        if (lastBlock >= pendingBlockNumber) {
            pendingBlock = await this.getBlockByNumber("pending");
        }
        // This is part of a temporary fix to https://github.com/NomicFoundation/hardhat/issues/2380
        const firstLocalBlock = !rangeIncludesRemoteBlocks
            ? oldestBlock
            : BigIntUtils.min(BigInt(this._forkBlockNumber), lastBlock) + 1n;
        for (let blockNumber = firstLocalBlock; // This is part of a temporary fix to https://github.com/NomicFoundation/hardhat/issues/2380
         blockNumber <= lastBlock; blockNumber++) {
            if (blockNumber < pendingBlockNumber) {
                // We know the block exists
                const block = (await this.getBlockByNumber(blockNumber));
                baseFeePerGas.push(block.header.baseFeePerGas ?? 0n);
                if (blockNumber < lastBlock) {
                    gasUsedRatio.push(this._getGasUsedRatio(block));
                    if (rewardPercentiles.length > 0) {
                        reward.push(await this._getRewards(block, rewardPercentiles));
                    }
                }
            }
            else if (blockNumber === pendingBlockNumber) {
                // This can only be run with EIP-1559, so this exists
                baseFeePerGas.push((await this.getNextBlockBaseFeePerGas()));
                if (blockNumber < lastBlock) {
                    gasUsedRatio.push(this._getGasUsedRatio(pendingBlock));
                    if (rewardPercentiles.length > 0) {
                        // We don't compute this for the pending block, as there's no
                        // effective miner fee yet.
                        reward.push(rewardPercentiles.map((_) => 0n));
                    }
                }
            }
            else if (blockNumber === pendingBlockNumber + 1n) {
                baseFeePerGas.push(pendingBlock.header.calcNextBaseFee());
            }
            else {
                (0, errors_1.assertHardhatInvariant)(false, "This should never happen");
            }
        }
        return {
            oldestBlock,
            baseFeePerGas,
            gasUsedRatio,
            reward: rewardPercentiles.length > 0 ? reward : undefined,
        };
    }
    async setCoinbase(coinbase) {
        this._coinbase = coinbase.toString();
    }
    _getGasUsedRatio(block) {
        const FLOATS_PRECISION = 100000;
        return (Number((block.header.gasUsed * BigInt(FLOATS_PRECISION)) /
            block.header.gasLimit) / FLOATS_PRECISION);
    }
    async _getRewards(block, rewardPercentiles) {
        const FLOATS_PRECISION = 100000;
        if (block.transactions.length === 0) {
            return rewardPercentiles.map((_) => 0n);
        }
        const receipts = await Promise.all(block.transactions
            .map((tx) => tx.hash())
            .map((hash) => this.getTransactionReceipt(hash)));
        const effectiveGasRewardAndGas = receipts
            .map((r, i) => {
            const tx = block.transactions[i];
            const baseFeePerGas = block.header.baseFeePerGas ?? 0n;
            // reward = min(maxPriorityFeePerGas, maxFeePerGas - baseFeePerGas)
            let effectiveGasReward;
            if ("maxPriorityFeePerGas" in tx) {
                effectiveGasReward = tx.maxFeePerGas - baseFeePerGas;
                if (tx.maxPriorityFeePerGas < effectiveGasReward) {
                    effectiveGasReward = tx.maxPriorityFeePerGas;
                }
            }
            else {
                effectiveGasReward = tx.gasPrice - baseFeePerGas;
            }
            return {
                effectiveGasReward,
                gasUsed: (0, base_types_1.rpcQuantityToBigInt)(r?.gasUsed),
            };
        })
            .sort((a, b) => BigIntUtils.cmp(a.effectiveGasReward, b.effectiveGasReward));
        return rewardPercentiles.map((p) => {
            let gasUsed = 0n;
            const targetGas = (block.header.gasLimit * BigInt(Math.ceil(p * FLOATS_PRECISION))) /
                BigInt(100 * FLOATS_PRECISION);
            for (const values of effectiveGasRewardAndGas) {
                gasUsed += values.gasUsed;
                if (targetGas <= gasUsed) {
                    return values.effectiveGasReward;
                }
            }
            return effectiveGasRewardAndGas[effectiveGasRewardAndGas.length - 1]
                .effectiveGasReward;
        });
    }
    async _addPendingTransaction(tx) {
        await this._txPool.addTransaction(tx);
        await this._notifyPendingTransaction(tx);
        return (0, ethereumjs_util_1.bufferToHex)(tx.hash());
    }
    async _mineTransaction(tx) {
        await this._addPendingTransaction(tx);
        return this.mineBlock();
    }
    async _mineTransactionAndPending(tx) {
        const snapshotId = await this.takeSnapshot();
        let result;
        try {
            const txHash = await this._addPendingTransaction(tx);
            result = await this._mineBlocksUntilTransactionIsIncluded(txHash);
        }
        catch (err) {
            await this.revertToSnapshot(snapshotId);
            throw err;
        }
        this._removeSnapshot(snapshotId);
        return result;
    }
    async _mineBlocksUntilTransactionIsIncluded(txHash) {
        const results = [];
        let txReceipt;
        do {
            if (!this._txPool.hasPendingTransactions()) {
                throw new errors_2.TransactionExecutionError("Failed to mine transaction for unknown reason, this should never happen");
            }
            results.push(await this.mineBlock());
            txReceipt = await this.getTransactionReceipt(txHash);
        } while (txReceipt === undefined);
        while (this._txPool.hasPendingTransactions()) {
            results.push(await this.mineBlock());
        }
        return results;
    }
    async _gatherTraces(result) {
        let vmTrace = this._vmTracer.getLastTopLevelMessageTrace();
        const vmTracerError = this._vmTracer.getLastError();
        this._vmTracer.clearLastError();
        if (vmTrace !== undefined) {
            vmTrace = this._vmTraceDecoder.tryToDecodeMessageTrace(vmTrace);
        }
        const consoleLogMessages = await this._getConsoleLogMessages(vmTrace, vmTracerError);
        const error = await this._manageErrors(result, vmTrace, vmTracerError);
        return {
            trace: vmTrace,
            consoleLogMessages,
            error,
        };
    }
    async _validateAutominedTx(tx) {
        let sender;
        try {
            sender = tx.getSenderAddress(); // verifies signature as a side effect
        }
        catch (e) {
            if (e instanceof Error) {
                throw new errors_2.InvalidInputError(e.message);
            }
            // eslint-disable-next-line @nomicfoundation/hardhat-internal-rules/only-hardhat-error
            throw e;
        }
        // validate nonce
        const nextPendingNonce = await this._txPool.getNextPendingNonce(sender);
        const txNonce = tx.nonce;
        const expectedNonceMsg = `Expected nonce to be ${nextPendingNonce.toString()} but got ${txNonce.toString()}.`;
        if (txNonce > nextPendingNonce) {
            throw new errors_2.InvalidInputError(`Nonce too high. ${expectedNonceMsg} Note that transactions can't be queued when automining.`);
        }
        if (txNonce < nextPendingNonce) {
            throw new errors_2.InvalidInputError(`Nonce too low. ${expectedNonceMsg}`);
        }
        // validate gas price
        const txPriorityFee = "gasPrice" in tx ? tx.gasPrice : tx.maxPriorityFeePerGas;
        if (txPriorityFee < this._minGasPrice) {
            throw new errors_2.InvalidInputError(`Transaction gas price is ${txPriorityFee.toString()}, which is below the minimum of ${this._minGasPrice.toString()}`);
        }
        // Validate that maxFeePerGas >= next block's baseFee
        const nextBlockGasFee = await this.getNextBlockBaseFeePerGas();
        if (nextBlockGasFee !== undefined) {
            if ("maxFeePerGas" in tx) {
                if (nextBlockGasFee > tx.maxFeePerGas) {
                    throw new errors_2.InvalidInputError(`Transaction maxFeePerGas (${tx.maxFeePerGas.toString()}) is too low for the next block, which has a baseFeePerGas of ${nextBlockGasFee.toString()}`);
                }
            }
            else {
                if (nextBlockGasFee > tx.gasPrice) {
                    throw new errors_2.InvalidInputError(`Transaction gasPrice (${tx.gasPrice.toString()}) is too low for the next block, which has a baseFeePerGas of ${nextBlockGasFee.toString()}`);
                }
            }
        }
    }
    /**
     * Mines a new block with as many pending txs as possible, adding it to
     * the VM's blockchain.
     *
     * This method reverts any modification to the state manager if it throws.
     */
    async _mineBlockWithPendingTxs(blockTimestamp) {
        const parentBlock = await this.getLatestBlock();
        const headerData = {
            gasLimit: this.getBlockGasLimit(),
            coinbase: this.getCoinbaseAddress(),
            nonce: this.isPostMergeHardfork()
                ? "0x0000000000000000"
                : "0x0000000000000042",
            timestamp: blockTimestamp,
        };
        if (this.isPostMergeHardfork()) {
            headerData.mixHash = this._getNextMixHash();
        }
        headerData.baseFeePerGas = await this.getNextBlockBaseFeePerGas();
        const blockBuilder = await this._vm.buildBlock({
            parentBlock,
            headerData,
            blockOpts: { calcDifficultyFromHeader: parentBlock.header },
        });
        try {
            const traces = [];
            const blockGasLimit = this.getBlockGasLimit();
            const minTxFee = this._getMinimalTransactionFee();
            const pendingTxs = this._txPool.getPendingTransactions();
            const transactionQueue = new TransactionQueue_1.TransactionQueue(pendingTxs, this._mempoolOrder, headerData.baseFeePerGas);
            let tx = transactionQueue.getNextTransaction();
            const results = [];
            const receipts = [];
            while (blockGasLimit - blockBuilder.gasUsed >= minTxFee &&
                tx !== undefined) {
                if (!this._isTxMinable(tx, headerData.baseFeePerGas) ||
                    tx.gasLimit > blockGasLimit - blockBuilder.gasUsed) {
                    transactionQueue.removeLastSenderTransactions();
                }
                else {
                    const txResult = await blockBuilder.addTransaction(tx);
                    traces.push(await this._gatherTraces(txResult.execResult));
                    results.push(txResult);
                    receipts.push(txResult.receipt);
                }
                tx = transactionQueue.getNextTransaction();
            }
            const block = await blockBuilder.build();
            await this._txPool.updatePendingAndQueued();
            return {
                block,
                blockResult: {
                    results,
                    receipts,
                    stateRoot: block.header.stateRoot,
                    logsBloom: block.header.logsBloom,
                    receiptsRoot: block.header.receiptTrie,
                    gasUsed: block.header.gasUsed,
                },
                traces,
            };
        }
        catch (err) {
            await blockBuilder.revert();
            throw err;
        }
    }
    _getMinimalTransactionFee() {
        // Typically 21_000 gas
        return this._vm._common.param("gasPrices", "tx");
    }
    async _getFakeTransaction(txParams) {
        const sender = new ethereumjs_util_1.Address(txParams.from);
        if ("maxFeePerGas" in txParams && txParams.maxFeePerGas !== undefined) {
            return new FakeSenderEIP1559Transaction_1.FakeSenderEIP1559Transaction(sender, txParams, {
                common: this._vm._common,
            });
        }
        if ("accessList" in txParams && txParams.accessList !== undefined) {
            return new FakeSenderAccessListEIP2930Transaction_1.FakeSenderAccessListEIP2930Transaction(sender, txParams, {
                common: this._vm._common,
            });
        }
        return new FakeSenderTransaction_1.FakeSenderTransaction(sender, txParams, {
            common: this._vm._common,
        });
    }
    _getSnapshotIndex(id) {
        for (const [i, snapshot] of this._snapshots.entries()) {
            if (snapshot.id === id) {
                return i;
            }
            // We already removed the snapshot we are looking for
            if (snapshot.id > id) {
                return undefined;
            }
        }
        return undefined;
    }
    _removeSnapshot(id) {
        const snapshotIndex = this._getSnapshotIndex(id);
        if (snapshotIndex === undefined) {
            return;
        }
        this._snapshots.splice(snapshotIndex);
    }
    _initLocalAccounts(genesisAccounts) {
        const privateKeys = genesisAccounts.map((acc) => (0, ethereumjs_util_1.toBuffer)(acc.privateKey));
        for (const pk of privateKeys) {
            this._localAccounts.set((0, ethereumjs_util_1.bufferToHex)((0, ethereumjs_util_1.privateToAddress)(pk)), pk);
        }
    }
    async _getConsoleLogMessages(vmTrace, vmTracerError) {
        if (vmTrace === undefined || vmTracerError !== undefined) {
            log("Could not print console log. Please report this to help us improve Hardhat.\n", vmTracerError);
            return [];
        }
        return this._consoleLogger.getLogMessages(vmTrace);
    }
    async _manageErrors(vmResult, vmTrace, vmTracerError) {
        if (vmResult.exceptionError === undefined) {
            return undefined;
        }
        let stackTrace;
        try {
            if (vmTrace === undefined || vmTracerError !== undefined) {
                throw vmTracerError;
            }
            stackTrace = this._solidityTracer.getStackTrace(vmTrace);
        }
        catch (err) {
            this._failedStackTraces += 1;
            log("Could not generate stack trace. Please report this to help us improve Hardhat.\n", err);
        }
        const error = vmResult.exceptionError;
        // we don't use `instanceof` in case someone uses a different VM dependency
        // see https://github.com/nomiclabs/hardhat/issues/1317
        const isVmError = "error" in error && typeof error.error === "string";
        // If this is not a VM error, or if it's an internal VM error, we just
        // rethrow. An example of a non-VmError being thrown here is an HTTP error
        // coming from the ForkedStateManager.
        if (!isVmError || error.error === exceptions_1.ERROR.INTERNAL_ERROR) {
            throw error;
        }
        if (error.error === exceptions_1.ERROR.CODESIZE_EXCEEDS_MAXIMUM) {
            if (stackTrace !== undefined) {
                return (0, solidity_errors_1.encodeSolidityStackTrace)("Transaction ran out of gas", stackTrace);
            }
            return new errors_2.TransactionExecutionError("Transaction ran out of gas");
        }
        if (error.error === exceptions_1.ERROR.OUT_OF_GAS) {
            // if the error is an out of gas, we ignore the inferred error in the
            // trace
            return new errors_2.TransactionExecutionError("Transaction ran out of gas");
        }
        const returnData = new return_data_1.ReturnData(vmResult.returnValue);
        let returnDataExplanation;
        if (returnData.isEmpty()) {
            returnDataExplanation = "without reason string";
        }
        else if (returnData.isErrorReturnData()) {
            returnDataExplanation = `with reason "${returnData.decodeError()}"`;
        }
        else if (returnData.isPanicReturnData()) {
            const panicCode = returnData.decodePanic().toString(16);
            returnDataExplanation = `with panic code "0x${panicCode}"`;
        }
        else {
            returnDataExplanation = "with unrecognized return data or custom error";
        }
        if (error.error === exceptions_1.ERROR.REVERT) {
            const fallbackMessage = `VM Exception while processing transaction: revert ${returnDataExplanation}`;
            if (stackTrace !== undefined) {
                return (0, solidity_errors_1.encodeSolidityStackTrace)(fallbackMessage, stackTrace);
            }
            return new errors_2.TransactionExecutionError(fallbackMessage);
        }
        if (stackTrace !== undefined) {
            return (0, solidity_errors_1.encodeSolidityStackTrace)(`Transaction failed: revert ${returnDataExplanation}`, stackTrace);
        }
        return new errors_2.TransactionExecutionError(`Transaction reverted ${returnDataExplanation}`);
    }
    _calculateTimestampAndOffset(timestamp) {
        let blockTimestamp;
        let offsetShouldChange;
        let newOffset = 0n;
        const currentTimestamp = BigInt((0, getCurrentTimestamp_1.getCurrentTimestamp)());
        // if timestamp is not provided, we check nextBlockTimestamp, if it is
        // set, we use it as the timestamp instead. If it is not set, we use
        // time offset + real time as the timestamp.
        if (timestamp === undefined || timestamp === 0n) {
            if (this.getNextBlockTimestamp() === 0n) {
                blockTimestamp = currentTimestamp + this.getTimeIncrement();
                offsetShouldChange = false;
            }
            else {
                blockTimestamp = this.getNextBlockTimestamp();
                offsetShouldChange = true;
            }
        }
        else {
            offsetShouldChange = true;
            blockTimestamp = timestamp;
        }
        if (offsetShouldChange) {
            newOffset = blockTimestamp - currentTimestamp;
        }
        return [blockTimestamp, offsetShouldChange, newOffset];
    }
    _resetNextBlockTimestamp() {
        this.setNextBlockTimestamp(0n);
    }
    async _notifyPendingTransaction(tx) {
        this._filters.forEach((filter) => {
            if (filter.type === filter_1.Type.PENDING_TRANSACTION_SUBSCRIPTION) {
                const hash = (0, ethereumjs_util_1.bufferToHex)(tx.hash());
                if (filter.subscription) {
                    this._emitEthEvent(filter.id, hash);
                    return;
                }
                filter.hashes.push(hash);
            }
        });
    }
    _getLocalAccountPrivateKey(sender) {
        const senderAddress = sender.toString();
        if (!this._localAccounts.has(senderAddress)) {
            throw new errors_2.InvalidInputError(`unknown account ${senderAddress}`);
        }
        return this._localAccounts.get(senderAddress);
    }
    /**
     * Saves a block as successfully run. This method requires that the block
     * was added to the blockchain.
     */
    async _saveBlockAsSuccessfullyRun(block, runBlockResult) {
        const receipts = (0, output_1.getRpcReceiptOutputsFromLocalBlockExecution)(block, runBlockResult, (0, output_1.shouldShowTransactionTypeForHardfork)(this._vm._common));
        this._blockchain.addTransactionReceipts(receipts);
        const td = await this.getBlockTotalDifficulty(block);
        const rpcLogs = [];
        for (const receipt of receipts) {
            rpcLogs.push(...receipt.logs);
        }
        this._filters.forEach((filter, key) => {
            if (filter.deadline.valueOf() < new Date().valueOf()) {
                this._filters.delete(key);
            }
            switch (filter.type) {
                case filter_1.Type.BLOCK_SUBSCRIPTION:
                    const hash = block.hash();
                    if (filter.subscription) {
                        this._emitEthEvent(filter.id, (0, output_1.getRpcBlock)(block, td, (0, output_1.shouldShowTransactionTypeForHardfork)(this._vm._common), false));
                        return;
                    }
                    filter.hashes.push((0, ethereumjs_util_1.bufferToHex)(hash));
                    break;
                case filter_1.Type.LOGS_SUBSCRIPTION:
                    if ((0, filter_1.bloomFilter)(new ethereumjs_vm_1.Bloom(block.header.logsBloom), filter.criteria.addresses, filter.criteria.normalizedTopics)) {
                        const logs = (0, filter_1.filterLogs)(rpcLogs, filter.criteria);
                        if (logs.length === 0) {
                            return;
                        }
                        if (filter.subscription) {
                            logs.forEach((rpcLog) => {
                                this._emitEthEvent(filter.id, rpcLog);
                            });
                            return;
                        }
                        filter.logs.push(...logs);
                    }
                    break;
            }
        });
    }
    async _timestampClashesWithPreviousBlockOne(blockTimestamp) {
        const latestBlock = await this.getLatestBlock();
        const latestBlockTimestamp = latestBlock.header.timestamp;
        return latestBlockTimestamp === blockTimestamp;
    }
    async _runInBlockContext(blockNumberOrPending, action) {
        if (blockNumberOrPending === "pending") {
            return this._runInPendingBlockContext(action);
        }
        if (blockNumberOrPending === this.getLatestBlockNumber()) {
            return action();
        }
        const block = await this.getBlockByNumber(blockNumberOrPending);
        if (block === undefined) {
            // TODO handle this better
            throw new Error(`Block with number ${blockNumberOrPending.toString()} doesn't exist. This should never happen.`);
        }
        const currentStateRoot = await this._stateManager.getStateRoot();
        await this._setBlockContext(block);
        try {
            return await action();
        }
        finally {
            await this._restoreBlockContext(currentStateRoot);
        }
    }
    async _runInPendingBlockContext(action) {
        const snapshotId = await this.takeSnapshot();
        try {
            await this.mineBlock();
            return await action();
        }
        finally {
            await this.revertToSnapshot(snapshotId);
        }
    }
    async _setBlockContext(block) {
        const irregularStateOrUndefined = this._irregularStatesByBlockNumber.get(block.header.number);
        if (this._stateManager instanceof ForkStateManager_1.ForkStateManager) {
            return this._stateManager.setBlockContext(block.header.stateRoot, block.header.number, irregularStateOrUndefined);
        }
        return this._stateManager.setStateRoot(irregularStateOrUndefined ?? block.header.stateRoot);
    }
    async _restoreBlockContext(stateRoot) {
        if (this._stateManager instanceof ForkStateManager_1.ForkStateManager) {
            return this._stateManager.restoreForkBlockContext(stateRoot);
        }
        return this._stateManager.setStateRoot(stateRoot);
    }
    async _correctInitialEstimation(blockNumberOrPending, txParams, initialEstimation) {
        let tx = await this._getFakeTransaction({
            ...txParams,
            gasLimit: initialEstimation,
        });
        if (tx.getBaseFee() >= initialEstimation) {
            initialEstimation = tx.getBaseFee() + 1n;
            tx = await this._getFakeTransaction({
                ...txParams,
                gasLimit: initialEstimation,
            });
        }
        const result = await this._runInBlockContext(blockNumberOrPending, () => this._runTxAndRevertMutations(tx, blockNumberOrPending));
        if (result.execResult.exceptionError === undefined) {
            return initialEstimation;
        }
        return this._binarySearchEstimation(blockNumberOrPending, txParams, initialEstimation, this.getBlockGasLimit());
    }
    async _binarySearchEstimation(blockNumberOrPending, txParams, highestFailingEstimation, lowestSuccessfulEstimation, roundNumber = 0) {
        if (lowestSuccessfulEstimation <= highestFailingEstimation) {
            // This shouldn't happen, but we don't want to go into an infinite loop
            // if it ever happens
            return lowestSuccessfulEstimation;
        }
        const MAX_GAS_ESTIMATION_IMPROVEMENT_ROUNDS = 20;
        const diff = lowestSuccessfulEstimation - highestFailingEstimation;
        const minDiff = highestFailingEstimation >= 4000000n
            ? 50000
            : highestFailingEstimation >= 1000000n
                ? 10000
                : highestFailingEstimation >= 100000n
                    ? 1000
                    : highestFailingEstimation >= 50000n
                        ? 500
                        : highestFailingEstimation >= 30000n
                            ? 300
                            : 200;
        if (diff <= minDiff) {
            return lowestSuccessfulEstimation;
        }
        if (roundNumber > MAX_GAS_ESTIMATION_IMPROVEMENT_ROUNDS) {
            return lowestSuccessfulEstimation;
        }
        const binSearchNewEstimation = highestFailingEstimation + diff / 2n;
        const optimizedEstimation = roundNumber === 0
            ? 3n * highestFailingEstimation
            : binSearchNewEstimation;
        const newEstimation = optimizedEstimation > binSearchNewEstimation
            ? binSearchNewEstimation
            : optimizedEstimation;
        // Let other things execute
        await new Promise((resolve) => setImmediate(resolve));
        const tx = await this._getFakeTransaction({
            ...txParams,
            gasLimit: newEstimation,
        });
        const result = await this._runInBlockContext(blockNumberOrPending, () => this._runTxAndRevertMutations(tx, blockNumberOrPending));
        if (result.execResult.exceptionError === undefined) {
            return this._binarySearchEstimation(blockNumberOrPending, txParams, highestFailingEstimation, newEstimation, roundNumber + 1);
        }
        return this._binarySearchEstimation(blockNumberOrPending, txParams, newEstimation, lowestSuccessfulEstimation, roundNumber + 1);
    }
    async _applyStateOverrideSet(stateOverrideSet) {
        // Multiple state override set can be configured for different addresses, hence the loop
        for (const [addrToOverride, stateOverrideOptions] of Object.entries(stateOverrideSet)) {
            const address = new ethereumjs_util_1.Address((0, ethereumjs_util_1.toBuffer)(addrToOverride));
            const { balance, nonce, code, state, stateDiff } = stateOverrideOptions;
            await this._overrideBalanceAndNonce(address, balance, nonce);
            await this._overrideCode(address, code);
            await this._overrideStateAndStateDiff(address, state, stateDiff);
        }
    }
    async _overrideBalanceAndNonce(address, balance, nonce) {
        const MAX_NONCE = 2n ** 64n - 1n;
        const MAX_BALANCE = 2n ** 256n - 1n;
        if (nonce !== undefined && nonce > MAX_NONCE) {
            throw new errors_2.InvalidInputError(`The 'nonce' property should occupy a maximum of 8 bytes (nonce=${nonce}).`);
        }
        if (balance !== undefined && balance > MAX_BALANCE) {
            throw new errors_2.InvalidInputError(`The 'balance' property should occupy a maximum of 32 bytes (balance=${balance}).`);
        }
        await this._stateManager.modifyAccountFields(address, {
            balance,
            nonce,
        });
    }
    async _overrideCode(address, code) {
        if (code === undefined)
            return;
        await this._stateManager.putContractCode(address, code);
    }
    async _overrideStateAndStateDiff(address, state, stateDiff) {
        let newState;
        if (state !== undefined && stateDiff === undefined) {
            await this._stateManager.clearContractStorage(address);
            newState = state;
        }
        else if (state === undefined && stateDiff !== undefined) {
            newState = stateDiff;
        }
        else if (state === undefined && stateDiff === undefined) {
            // nothing to do
            return;
        }
        else {
            throw new errors_2.InvalidInputError("The properties 'state' and 'stateDiff' cannot be used simultaneously when configuring the state override set passed to the eth_call method.");
        }
        for (const [storageKey, value] of Object.entries(newState)) {
            await this._stateManager.putContractStorage(address, (0, ethereumjs_util_1.toBuffer)(storageKey), (0, ethereumjs_util_1.setLengthLeft)((0, ethereumjs_util_1.bigIntToBuffer)(value), 32));
        }
    }
    /**
     * This function runs a transaction and reverts all the modifications that it
     * makes.
     */
    async _runTxAndRevertMutations(tx, blockNumberOrPending, forceBaseFeeZero = false, stateOverrideSet = {}) {
        const initialStateRoot = await this._stateManager.getStateRoot();
        await this._applyStateOverrideSet(stateOverrideSet);
        let blockContext;
        let originalCommon;
        try {
            if (blockNumberOrPending === "pending") {
                // the new block has already been mined by _runInBlockContext hence we take latest here
                blockContext = await this.getLatestBlock();
            }
            else {
                // We know that this block number exists, because otherwise
                // there would be an error in the RPC layer.
                const block = await this.getBlockByNumber(blockNumberOrPending);
                (0, errors_1.assertHardhatInvariant)(block !== undefined, "Tried to run a tx in the context of a non-existent block");
                blockContext = block;
                // we don't need to add the tx to the block because runTx doesn't
                // know anything about the txs in the current block
            }
            originalCommon = this._vm._common;
            this._vm._common = ethereumjs_common_1.Common.custom({
                chainId: this._forkBlockNumber === undefined ||
                    blockContext.header.number >= this._forkBlockNumber
                    ? this._configChainId
                    : this._forkNetworkId,
                networkId: this._forkNetworkId ?? this._configNetworkId,
            }, {
                hardfork: this._selectHardfork(blockContext.header.number),
                ...this._getTransientStorageSettings(),
            });
            // If this VM is running without EIP4895, but the block has withdrawals,
            // we remove them and the withdrawal root from the block
            if (!this.isEip4895Active(blockNumberOrPending) &&
                blockContext.withdrawals !== undefined) {
                blockContext = ethereumjs_block_1.Block.fromBlockData({
                    ...blockContext,
                    withdrawals: undefined,
                    header: {
                        ...blockContext.header,
                        withdrawalsRoot: undefined,
                    },
                }, {
                    freeze: false,
                    common: this._vm._common,
                    skipConsensusFormatValidation: true,
                });
            }
            // NOTE: This is a workaround of both an @nomicfoundation/ethereumjs-vm limitation, and
            //   a bug in Hardhat Network.
            //
            // See: https://github.com/nomiclabs/hardhat/issues/1666
            //
            // If this VM is running with EIP1559 activated, and the block is not
            // an EIP1559 one, this will crash, so we create a new one that has
            // baseFeePerGas = 0.
            //
            // We also have an option to force the base fee to be zero,
            // we don't want to debit any balance nor fail any tx when running an
            // eth_call. This will make the BASEFEE option also return 0, which
            // shouldn't. See: https://github.com/nomiclabs/hardhat/issues/1688
            if (this.isEip1559Active(blockNumberOrPending) &&
                (blockContext.header.baseFeePerGas === undefined || forceBaseFeeZero)) {
                blockContext = ethereumjs_block_1.Block.fromBlockData(blockContext, {
                    freeze: false,
                    common: this._vm._common,
                    skipConsensusFormatValidation: true,
                });
                blockContext.header.baseFeePerGas = 0n;
            }
            return await this._vm.runTx({
                block: blockContext,
                tx,
                skipNonce: true,
                skipBalance: true,
                skipBlockGasLimitValidation: true,
                skipHardForkValidation: true,
            });
        }
        finally {
            if (originalCommon !== undefined) {
                this._vm._common = originalCommon;
            }
            await this._stateManager.setStateRoot(initialStateRoot);
        }
    }
    async _computeFilterParams(filterParams, isFilter) {
        const latestBlockNumber = this.getLatestBlockNumber();
        const newFilterParams = { ...filterParams };
        if (newFilterParams.fromBlock === filter_1.LATEST_BLOCK) {
            newFilterParams.fromBlock = latestBlockNumber;
        }
        if (!isFilter && newFilterParams.toBlock === filter_1.LATEST_BLOCK) {
            newFilterParams.toBlock = latestBlockNumber;
        }
        if (newFilterParams.toBlock > latestBlockNumber) {
            newFilterParams.toBlock = latestBlockNumber;
        }
        if (newFilterParams.fromBlock > latestBlockNumber) {
            newFilterParams.fromBlock = latestBlockNumber;
        }
        return newFilterParams;
    }
    _newDeadline() {
        const dt = new Date();
        dt.setMinutes(dt.getMinutes() + 5); // This will not overflow
        return dt;
    }
    _getNextFilterId() {
        this._lastFilterId++;
        return this._lastFilterId;
    }
    _filterIdToFiltersKey(filterId) {
        return filterId.toString();
    }
    _emitEthEvent(filterId, result) {
        this.emit("ethEvent", {
            result,
            filterId,
        });
    }
    async _getNonce(address, blockNumberOrPending) {
        if (blockNumberOrPending === "pending") {
            return this.getAccountNextPendingNonce(address);
        }
        return this._runInBlockContext(blockNumberOrPending, async () => {
            const account = await this._stateManager.getAccount(address);
            return account.nonce;
        });
    }
    async _isTransactionMined(hash) {
        const txReceipt = await this.getTransactionReceipt(hash);
        return txReceipt !== undefined;
    }
    _isTxMinable(tx, nextBlockBaseFeePerGas) {
        const txMaxFee = "gasPrice" in tx ? tx.gasPrice : tx.maxFeePerGas;
        const canPayBaseFee = nextBlockBaseFeePerGas !== undefined
            ? txMaxFee >= nextBlockBaseFeePerGas
            : true;
        const atLeastMinGasPrice = txMaxFee >= this._minGasPrice;
        return canPayBaseFee && atLeastMinGasPrice;
    }
    async _persistIrregularWorldState() {
        this._irregularStatesByBlockNumber.set(this.getLatestBlockNumber(), await this._stateManager.getStateRoot());
    }
    isEip1559Active(blockNumberOrPending) {
        if (blockNumberOrPending !== undefined &&
            blockNumberOrPending !== "pending") {
            return this._vm._common.hardforkGteHardfork(this._selectHardfork(blockNumberOrPending), "london");
        }
        return this._vm._common.gteHardfork("london");
    }
    isEip4895Active(blockNumberOrPending) {
        if (blockNumberOrPending !== undefined &&
            blockNumberOrPending !== "pending") {
            return this._vm._common.hardforkGteHardfork(this._selectHardfork(blockNumberOrPending), "shanghai");
        }
        return this._vm._common.gteHardfork("shanghai");
    }
    isPostMergeHardfork() {
        return (0, hardforks_1.hardforkGte)(this.hardfork, hardforks_1.HardforkName.MERGE);
    }
    setPrevRandao(prevRandao) {
        this._mixHashGenerator.setNext(prevRandao);
    }
    async getClientVersion() {
        const hardhatPackage = await (0, packageInfo_1.getPackageJson)();
        const ethereumjsVMPackage = require("@nomicfoundation/ethereumjs-vm/package.json");
        return `HardhatNetwork/${hardhatPackage.version}/@nomicfoundation/ethereumjs-vm/${ethereumjsVMPackage.version}`;
    }
    async getMetadata() {
        const clientVersion = await this.getClientVersion();
        const instanceIdHex = BigIntUtils.toEvmWord(this._instanceId);
        const instanceId = `0x${instanceIdHex}`;
        const latestBlock = await this.getLatestBlock();
        const latestBlockHashHex = latestBlock.header.hash().toString("hex");
        const latestBlockHash = `0x${latestBlockHashHex}`;
        const metadata = {
            clientVersion,
            chainId: this._configChainId,
            instanceId,
            latestBlockNumber: Number(latestBlock.header.number),
            latestBlockHash,
        };
        if (this._forkBlockNumber !== undefined) {
            (0, errors_1.assertHardhatInvariant)(this._forkNetworkId !== undefined, "this._forkNetworkId should be defined if this._forkBlockNumber is defined");
            (0, errors_1.assertHardhatInvariant)(this._forkBlockHash !== undefined, "this._forkBlockhash should be defined if this._forkBlockNumber is defined");
            metadata.forkedNetwork = {
                chainId: this._forkNetworkId,
                forkBlockNumber: Number(this._forkBlockNumber),
                forkBlockHash: this._forkBlockHash,
            };
        }
        return metadata;
    }
    _getNextMixHash() {
        return this._mixHashGenerator.next();
    }
    async _getEstimateGasFeePriceFields(callParams, blockNumberOrPending) {
        if (!this.isEip1559Active(blockNumberOrPending) ||
            callParams.gasPrice !== undefined) {
            return { gasPrice: callParams.gasPrice ?? (await this.getGasPrice()) };
        }
        let maxFeePerGas = callParams.maxFeePerGas;
        let maxPriorityFeePerGas = callParams.maxPriorityFeePerGas;
        if (maxPriorityFeePerGas === undefined) {
            maxPriorityFeePerGas = await this.getMaxPriorityFeePerGas();
            if (maxFeePerGas !== undefined && maxFeePerGas < maxPriorityFeePerGas) {
                maxPriorityFeePerGas = maxFeePerGas;
            }
        }
        if (maxFeePerGas === undefined) {
            if (blockNumberOrPending === "pending") {
                const baseFeePerGas = await this.getNextBlockBaseFeePerGas();
                maxFeePerGas = 2n * baseFeePerGas + maxPriorityFeePerGas;
            }
            else {
                const block = await this.getBlockByNumber(blockNumberOrPending);
                maxFeePerGas =
                    maxPriorityFeePerGas + (block.header.baseFeePerGas ?? 0n);
            }
        }
        return { maxFeePerGas, maxPriorityFeePerGas };
    }
    _selectHardfork(blockNumber) {
        if (this._forkBlockNumber === undefined ||
            blockNumber >= this._forkBlockNumber) {
            return this._vm._common.hardfork();
        }
        if (this._hardforkActivations.size === 0) {
            throw new errors_2.InternalError(`No known hardfork for execution on historical block ${blockNumber.toString()} (relative to fork block number ${this._forkBlockNumber}). The node was not configured with a hardfork activation history.  See http://hardhat.org/custom-hardfork-history`);
        }
        /** search this._hardforkActivations for the highest block number that
         * isn't higher than blockNumber, and then return that found block number's
         * associated hardfork name. */
        const hardforkHistory = Array.from(this._hardforkActivations.entries());
        const [hardfork, activationBlock] = hardforkHistory.reduce(([highestHardfork, highestBlock], [thisHardfork, thisBlock]) => thisBlock > highestBlock && thisBlock <= blockNumber
            ? [thisHardfork, thisBlock]
            : [highestHardfork, highestBlock]);
        if (hardfork === undefined || blockNumber < activationBlock) {
            throw new errors_2.InternalError(`Could not find a hardfork to run for block ${blockNumber.toString()}, after having looked for one in the HardhatNode's hardfork activation history, which was: ${JSON.stringify(hardforkHistory)}. For more information, see https://hardhat.org/hardhat-network/reference/#config`);
        }
        if (!constants_1.HARDHAT_NETWORK_SUPPORTED_HARDFORKS.includes(hardfork)) {
            throw new errors_2.InternalError(`Tried to run a call or transaction in the context of a block whose hardfork is "${hardfork}", but Hardhat Network only supports the following hardforks: ${constants_1.HARDHAT_NETWORK_SUPPORTED_HARDFORKS.join(", ")}`);
        }
        return hardfork;
    }
    _getCommonForTracing(networkId, blockNumber) {
        try {
            const common = ethereumjs_common_1.Common.custom({
                chainId: networkId,
                networkId,
            }, {
                hardfork: this._selectHardfork(BigInt(blockNumber)),
                ...this._getTransientStorageSettings(),
            });
            return common;
        }
        catch {
            throw new errors_2.InternalError(`Network id ${networkId} does not correspond to a network that Hardhat can trace`);
        }
    }
    _getTransientStorageSettings() {
        if (this._enableTransientStorage) {
            return { eips: [1153] };
        }
        return {};
    }
}
exports.HardhatNode = HardhatNode;
//# sourceMappingURL=node.js.map