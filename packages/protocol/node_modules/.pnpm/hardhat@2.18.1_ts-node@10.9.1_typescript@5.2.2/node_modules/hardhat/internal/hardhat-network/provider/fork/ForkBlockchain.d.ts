/// <reference types="node" />
import { Block } from "@nomicfoundation/ethereumjs-block";
import { Common } from "@nomicfoundation/ethereumjs-common";
import { TypedTransaction } from "@nomicfoundation/ethereumjs-tx";
import { JsonRpcClient } from "../../jsonrpc/client";
import { BlockchainBase } from "../BlockchainBase";
import { FilterParams } from "../node-types";
import { RpcLogOutput, RpcReceiptOutput } from "../output";
import { HardhatBlockchainInterface } from "../types/HardhatBlockchainInterface";
export declare class ForkBlockchain extends BlockchainBase implements HardhatBlockchainInterface {
    private _jsonRpcClient;
    private _forkBlockNumber;
    private _latestBlockNumber;
    constructor(_jsonRpcClient: JsonRpcClient, _forkBlockNumber: bigint, common: Common);
    getLatestBlockNumber(): bigint;
    getBlock(blockHashOrNumber: Buffer | bigint): Promise<Block>;
    addBlock(block: Block): Promise<Block>;
    reserveBlocks(count: bigint, interval: bigint, previousBlockStateRoot: Buffer, previousBlockTotalDifficulty: bigint, previousBlockBaseFeePerGas: bigint | undefined): void;
    deleteLaterBlocks(block: Block): void;
    getTotalDifficulty(blockHash: Buffer): Promise<bigint>;
    getTransaction(transactionHash: Buffer): Promise<TypedTransaction | undefined>;
    getBlockByTransactionHash(transactionHash: Buffer): Promise<Block | null>;
    getTransactionReceipt(transactionHash: Buffer): Promise<RpcReceiptOutput | null>;
    getForkBlockNumber(): bigint;
    getLogs(filterParams: FilterParams): Promise<RpcLogOutput[]>;
    private _getBlockByHash;
    private _getBlockByNumber;
    private _processRemoteBlock;
    protected _delBlock(blockNumber: bigint): void;
    private _processRemoteTransaction;
    private _processRemoteReceipt;
}
//# sourceMappingURL=ForkBlockchain.d.ts.map