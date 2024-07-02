/// <reference types="node" />
import { DB, BatchDBOp } from './db';
import type { LevelUp } from 'levelup';
export declare type Checkpoint = {
    keyValueMap: Map<string, Buffer | null>;
    root: Buffer;
};
/**
 * DB is a thin wrapper around the underlying levelup db,
 * which validates inputs and sets encoding type.
 */
export declare class CheckpointDB extends DB {
    checkpoints: Checkpoint[];
    /**
     * Initialize a DB instance. If `leveldb` is not provided, DB
     * defaults to an [in-memory store](https://github.com/Level/memdown).
     * @param leveldb - An abstract-leveldown compliant store
     */
    constructor(leveldb?: LevelUp);
    /**
     * Is the DB during a checkpoint phase?
     */
    get isCheckpoint(): boolean;
    /**
     * Adds a new checkpoint to the stack
     * @param root
     */
    checkpoint(root: Buffer): void;
    /**
     * Commits the latest checkpoint
     */
    commit(): Promise<void>;
    /**
     * Reverts the latest checkpoint
     */
    revert(): Promise<Buffer>;
    /**
     * Retrieves a raw value from leveldb.
     * @param key
     * @returns A Promise that resolves to `Buffer` if a value is found or `null` if no value is found.
     */
    get(key: Buffer): Promise<Buffer | null>;
    /**
     * Writes a value directly to leveldb.
     * @param key The key as a `Buffer`
     * @param value The value to be stored
     */
    put(key: Buffer, val: Buffer): Promise<void>;
    /**
     * Removes a raw value in the underlying leveldb.
     * @param keys
     */
    del(key: Buffer): Promise<void>;
    /**
     * Performs a batch operation on db.
     * @param opStack A stack of levelup operations
     */
    batch(opStack: BatchDBOp[]): Promise<void>;
}
