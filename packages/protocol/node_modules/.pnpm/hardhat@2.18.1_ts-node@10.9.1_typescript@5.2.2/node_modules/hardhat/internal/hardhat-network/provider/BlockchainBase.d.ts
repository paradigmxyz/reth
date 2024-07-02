/// <reference types="node" />
import { Block, BlockHeader } from "@nomicfoundation/ethereumjs-block";
import { BlockchainInterface, Consensus } from "@nomicfoundation/ethereumjs-blockchain";
import { Common } from "@nomicfoundation/ethereumjs-common";
import { TypedTransaction } from "@nomicfoundation/ethereumjs-tx";
import { BlockchainData } from "./BlockchainData";
import { RpcReceiptOutput } from "./output";
export declare abstract class BlockchainBase {
    protected _common: Common;
    consensus: Consensus;
    protected readonly _data: BlockchainData;
    constructor(_common: Common);
    abstract addBlock(block: Block): Promise<Block>;
    addTransactionReceipts(receipts: RpcReceiptOutput[]): void;
    delBlock(blockHash: Buffer): Promise<void>;
    deleteBlock(blockHash: Buffer): void;
    getBlock(blockHashOrNumber: Buffer | bigint | number): Promise<Block>;
    abstract getLatestBlockNumber(): bigint;
    getLatestBlock(): Promise<Block>;
    getLocalTransaction(transactionHash: Buffer): TypedTransaction | undefined;
    iterator(_name: string, _onBlock: (block: Block, reorg: boolean) => void | Promise<void>): Promise<number>;
    putBlock(block: Block): Promise<void>;
    reserveBlocks(count: bigint, interval: bigint, previousBlockStateRoot: Buffer, previousBlockTotalDifficulty: bigint, previousBlockBaseFeePerGas: bigint | undefined): void;
    copy(): BlockchainInterface;
    validateHeader(_header: BlockHeader, _height?: bigint | undefined): Promise<void>;
    protected _delBlock(blockNumber: bigint): void;
    protected _computeTotalDifficulty(block: Block): Promise<bigint>;
}
//# sourceMappingURL=BlockchainBase.d.ts.map