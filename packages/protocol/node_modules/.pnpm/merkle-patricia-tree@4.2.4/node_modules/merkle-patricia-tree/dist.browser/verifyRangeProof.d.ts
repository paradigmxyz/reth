/// <reference types="node" />
import { Nibbles } from './trieNode';
/**
 * verifyRangeProof checks whether the given leaf nodes and edge proof
 * can prove the given trie leaves range is matched with the specific root.
 *
 * There are four situations:
 *
 * - All elements proof. In this case the proof can be null, but the range should
 *   be all the leaves in the trie.
 *
 * - One element proof. In this case no matter the edge proof is a non-existent
 *   proof or not, we can always verify the correctness of the proof.
 *
 * - Zero element proof. In this case a single non-existent proof is enough to prove.
 *   Besides, if there are still some other leaves available on the right side, then
 *   an error will be returned.
 *
 * - Two edge elements proof. In this case two existent or non-existent proof(fisrt and last) should be provided.
 *
 * NOTE: Currently only supports verification when the length of firstKey and lastKey are the same.
 *
 * @param rootHash - root hash.
 * @param firstKey - first key.
 * @param lastKey - last key.
 * @param keys - key list.
 * @param values - value list, one-to-one correspondence with keys.
 * @param proof - proof node list, if proof is null, both `firstKey` and `lastKey` must be null
 * @returns a flag to indicate whether there exists more trie node in the trie
 */
export declare function verifyRangeProof(rootHash: Buffer, firstKey: Nibbles | null, lastKey: Nibbles | null, keys: Nibbles[], values: Buffer[], proof: Buffer[] | null): Promise<boolean>;
