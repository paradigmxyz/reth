import { Trie as BaseTrie } from './baseTrie';
import { CheckpointDB } from './checkpointDb';
/**
 * Adds checkpointing to the {@link BaseTrie}
 */
export declare class CheckpointTrie extends BaseTrie {
    db: CheckpointDB;
    constructor(...args: any);
    /**
     * Is the trie during a checkpoint phase?
     */
    get isCheckpoint(): boolean;
    /**
     * Creates a checkpoint that can later be reverted to or committed.
     * After this is called, all changes can be reverted until `commit` is called.
     */
    checkpoint(): void;
    /**
     * Commits a checkpoint to disk, if current checkpoint is not nested.
     * If nested, only sets the parent checkpoint as current checkpoint.
     * @throws If not during a checkpoint phase
     */
    commit(): Promise<void>;
    /**
     * Reverts the trie to the state it was at when `checkpoint` was first called.
     * If during a nested checkpoint, sets root to most recent checkpoint, and sets
     * parent checkpoint as current.
     */
    revert(): Promise<void>;
    /**
     * Returns a copy of the underlying trie with the interface of CheckpointTrie.
     * @param includeCheckpoints - If true and during a checkpoint, the copy will contain the checkpointing metadata and will use the same scratch as underlying db.
     */
    copy(includeCheckpoints?: boolean): CheckpointTrie;
}
