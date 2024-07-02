/// <reference types="node" />
import { Block } from "@nomicfoundation/ethereumjs-block";
import { Common } from "@nomicfoundation/ethereumjs-common";
import { TypedTransaction } from "@nomicfoundation/ethereumjs-tx";
import { FilterParams } from "./node-types";
import { RpcLogOutput, RpcReceiptOutput } from "./output";
interface Reservation {
    first: bigint;
    last: bigint;
    interval: bigint;
    previousBlockStateRoot: Buffer;
    previousBlockTotalDifficulty: bigint;
    previousBlockBaseFeePerGas: bigint | undefined;
}
export declare class BlockchainData {
    private _common;
    private _blocksByNumber;
    private _blocksByHash;
    private _blocksByTransactions;
    private _transactions;
    private _transactionReceipts;
    private _totalDifficulty;
    private _blockReservations;
    constructor(_common: Common);
    reserveBlocks(first: bigint, count: bigint, interval: bigint, previousBlockStateRoot: Buffer, previousBlockTotalDifficulty: bigint, previousBlockBaseFeePerGas: bigint | undefined): void;
    getBlockByNumber(blockNumber: bigint): Block | undefined;
    getBlockByHash(blockHash: Buffer): Block | undefined;
    getBlockByTransactionHash(transactionHash: Buffer): Block | undefined;
    getTransaction(transactionHash: Buffer): TypedTransaction | undefined;
    getTransactionReceipt(transactionHash: Buffer): RpcReceiptOutput | undefined;
    getTotalDifficulty(blockHash: Buffer): bigint | undefined;
    getLogs(filterParams: FilterParams): RpcLogOutput[];
    addBlock(block: Block, totalDifficulty: bigint): void;
    /**
     * WARNING: this method can leave the blockchain in an invalid state where
     * there are gaps between blocks. Ideally we should have a method that removes
     * the given block and all the following blocks.
     */
    removeBlock(block: Block): void;
    addTransaction(transaction: TypedTransaction): void;
    addTransactionReceipt(receipt: RpcReceiptOutput): void;
    isReservedBlock(blockNumber: bigint): boolean;
    private _findBlockReservation;
    /**
     * WARNING: this method only removes the given reservation and can result in
     * gaps in the reservations array. Ideally we should have a method that
     * removes the given reservation and all the following reservations.
     */
    private _removeReservation;
    /**
     * Cancel and return the reservation that has block `blockNumber`
     */
    cancelReservationWithBlock(blockNumber: bigint): Reservation;
    fulfillBlockReservation(blockNumber: bigint): void;
    private _calculateTimestampForReservedBlock;
}
export {};
//# sourceMappingURL=BlockchainData.d.ts.map