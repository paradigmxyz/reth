/// <reference types="node" />
import { Block } from "@nomicfoundation/ethereumjs-block";
import { Common } from "@nomicfoundation/ethereumjs-common";
import { TypedTransaction } from "@nomicfoundation/ethereumjs-tx";
import { BlockchainBase } from "./BlockchainBase";
import { FilterParams } from "./node-types";
import { RpcLogOutput } from "./output";
import { HardhatBlockchainInterface } from "./types/HardhatBlockchainInterface";
export declare class HardhatBlockchain extends BlockchainBase implements HardhatBlockchainInterface {
    private _length;
    constructor(common: Common);
    getLatestBlockNumber(): bigint;
    addBlock(block: Block): Promise<Block>;
    reserveBlocks(count: bigint, interval: bigint, previousBlockStateRoot: Buffer, previousBlockTotalDifficulty: bigint, previousBlockBaseFeePerGas: bigint | undefined): void;
    deleteLaterBlocks(block: Block): void;
    getTotalDifficulty(blockHash: Buffer): Promise<bigint>;
    getTransaction(transactionHash: Buffer): Promise<TypedTransaction | undefined>;
    getBlockByTransactionHash(transactionHash: Buffer): Promise<Block | null>;
    getTransactionReceipt(transactionHash: Buffer): Promise<import("./output").RpcReceiptOutput | null>;
    getLogs(filterParams: FilterParams): Promise<RpcLogOutput[]>;
    private _validateBlock;
    protected _delBlock(blockNumber: bigint): void;
}
//# sourceMappingURL=HardhatBlockchain.d.ts.map