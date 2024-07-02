import { BranchNode, ExtensionNode, LeafNode, Trie } from '../trie'
import { nibblesCompare, nibblesToBuffer } from '../util/nibbles'

import type { HashKeysFunction, Nibbles, TrieNode } from '../types'

// reference: https://github.com/ethereum/go-ethereum/blob/20356e57b119b4e70ce47665a71964434e15200d/trie/proof.go

/**
 * unset will remove all nodes to the left or right of the target key(decided by `removeLeft`).
 * @param trie - trie object.
 * @param parent - parent node, it can be `null`.
 * @param child - child node.
 * @param key - target nibbles.
 * @param pos - key position.
 * @param removeLeft - remove all nodes to the left or right of the target key.
 * @param stack - a stack of modified nodes.
 * @returns The end position of key.
 */
async function unset(
  trie: Trie,
  parent: TrieNode,
  child: TrieNode | null,
  key: Nibbles,
  pos: number,
  removeLeft: boolean,
  stack: TrieNode[]
): Promise<number> {
  if (child instanceof BranchNode) {
    /**
     * This node is a branch node,
     * remove all branches on the left or right
     */
    if (removeLeft) {
      for (let i = 0; i < key[pos]; i++) {
        child.setBranch(i, null)
      }
    } else {
      for (let i = key[pos] + 1; i < 16; i++) {
        child.setBranch(i, null)
      }
    }

    // record this node on the stack
    stack.push(child)

    // continue to the next node
    const next = child.getBranch(key[pos])
    const _child = next && (await trie.lookupNode(next))
    return unset(trie, child, _child, key, pos + 1, removeLeft, stack)
  } else if (child instanceof ExtensionNode || child instanceof LeafNode) {
    /**
     * This node is an extension node or lead node,
     * if node._nibbles is less or greater than the target key,
     * remove self from parent
     */
    if (
      key.length - pos < child.keyLength() ||
      nibblesCompare(child._nibbles, key.slice(pos, pos + child.keyLength())) !== 0
    ) {
      if (removeLeft) {
        if (nibblesCompare(child._nibbles, key.slice(pos)) < 0) {
          ;(parent as BranchNode).setBranch(key[pos - 1], null)
        }
      } else {
        if (nibblesCompare(child._nibbles, key.slice(pos)) > 0) {
          ;(parent as BranchNode).setBranch(key[pos - 1], null)
        }
      }
      return pos - 1
    }

    if (child instanceof LeafNode) {
      // This node is a leaf node, directly remove it from parent
      ;(parent as BranchNode).setBranch(key[pos - 1], null)
      return pos - 1
    } else {
      const _child = await trie.lookupNode(child.value())
      if (_child && _child instanceof LeafNode) {
        // The child of this node is leaf node, remove it from parent too
        ;(parent as BranchNode).setBranch(key[pos - 1], null)
        return pos - 1
      }

      // record this node on the stack
      stack.push(child)

      // continue to the next node
      return unset(trie, child, _child, key, pos + child.keyLength(), removeLeft, stack)
    }
  } else if (child === null) {
    return pos - 1
  } else {
    throw new Error('invalid node')
  }
}

/**
 * unsetInternal will remove all nodes between `left` and `right` (including `left` and `right`)
 * @param trie - trie object.
 * @param left - left nibbles.
 * @param right - right nibbles.
 * @returns Is it an empty trie.
 */
async function unsetInternal(trie: Trie, left: Nibbles, right: Nibbles): Promise<boolean> {
  // Key position
  let pos = 0
  // Parent node
  let parent: TrieNode | null = null
  // Current node
  let node: TrieNode | null = await trie.lookupNode(trie.root())
  let shortForkLeft!: number
  let shortForkRight!: number
  // A stack of modified nodes.
  const stack: TrieNode[] = []

  // 1. Find the fork point of `left` and `right`

  // eslint-disable-next-line no-constant-condition
  while (true) {
    if (node instanceof ExtensionNode || node instanceof LeafNode) {
      // record this node on the stack
      stack.push(node)

      if (left.length - pos < node.keyLength()) {
        shortForkLeft = nibblesCompare(left.slice(pos), node._nibbles)
      } else {
        shortForkLeft = nibblesCompare(left.slice(pos, pos + node.keyLength()), node._nibbles)
      }

      if (right.length - pos < node.keyLength()) {
        shortForkRight = nibblesCompare(right.slice(pos), node._nibbles)
      } else {
        shortForkRight = nibblesCompare(right.slice(pos, pos + node.keyLength()), node._nibbles)
      }

      // If one of `left` and `right` is not equal to node._nibbles, it means we found the fork point
      if (shortForkLeft !== 0 || shortForkRight !== 0) {
        break
      }

      if (node instanceof LeafNode) {
        // it shouldn't happen
        throw new Error('invalid node')
      }

      // continue to the next node
      parent = node
      pos += node.keyLength()
      node = await trie.lookupNode(node.value())
    } else if (node instanceof BranchNode) {
      // record this node on the stack
      stack.push(node)

      const leftNode = node.getBranch(left[pos])
      const rightNode = node.getBranch(right[pos])

      // One of `left` and `right` is `null`, stop searching
      if (leftNode === null || rightNode === null) {
        break
      }

      // Stop searching if `left` and `right` are not equal
      if (!(leftNode instanceof Buffer)) {
        if (rightNode instanceof Buffer) {
          break
        }

        if (leftNode.length !== rightNode.length) {
          break
        }

        let abort = false
        for (let i = 0; i < leftNode.length; i++) {
          if (leftNode[i].compare(rightNode[i]) !== 0) {
            abort = true
            break
          }
        }
        if (abort) {
          break
        }
      } else {
        if (!(rightNode instanceof Buffer)) {
          break
        }

        if (leftNode.compare(rightNode) !== 0) {
          break
        }
      }

      // continue to the next node
      parent = node
      node = await trie.lookupNode(leftNode)
      pos += 1
    } else {
      throw new Error('invalid node')
    }
  }

  // 2. Starting from the fork point, delete all nodes between `left` and `right`

  const saveStack = (key: Nibbles, stack: TrieNode[]) => {
    return trie._saveStack(key, stack, [])
  }

  if (node instanceof ExtensionNode || node instanceof LeafNode) {
    /**
     * There can have these five scenarios:
     * - both proofs are less than the trie path => no valid range
     * - both proofs are greater than the trie path => no valid range
     * - left proof is less and right proof is greater => valid range, unset the entire trie
     * - left proof points to the trie node, but right proof is greater => valid range, unset left node
     * - right proof points to the trie node, but left proof is less  => valid range, unset right node
     */
    const removeSelfFromParentAndSaveStack = async (key: Nibbles) => {
      if (parent === null) {
        return true
      }

      stack.pop()
      ;(parent as BranchNode).setBranch(key[pos - 1], null)
      await saveStack(key.slice(0, pos - 1), stack)
      return false
    }

    if (shortForkLeft === -1 && shortForkRight === -1) {
      throw new Error('invalid range')
    }

    if (shortForkLeft === 1 && shortForkRight === 1) {
      throw new Error('invalid range')
    }

    if (shortForkLeft !== 0 && shortForkRight !== 0) {
      // Unset the entire trie
      return removeSelfFromParentAndSaveStack(left)
    }

    // Unset left node
    if (shortForkRight !== 0) {
      if (node instanceof LeafNode) {
        return removeSelfFromParentAndSaveStack(left)
      }

      const child = await trie.lookupNode(node._value)
      if (child && child instanceof LeafNode) {
        return removeSelfFromParentAndSaveStack(left)
      }

      const endPos = await unset(trie, node, child, left.slice(pos), node.keyLength(), false, stack)
      await saveStack(left.slice(0, pos + endPos), stack)

      return false
    }

    // Unset right node
    if (shortForkLeft !== 0) {
      if (node instanceof LeafNode) {
        return removeSelfFromParentAndSaveStack(right)
      }

      const child = await trie.lookupNode(node._value)
      if (child && child instanceof LeafNode) {
        return removeSelfFromParentAndSaveStack(right)
      }

      const endPos = await unset(trie, node, child, right.slice(pos), node.keyLength(), true, stack)
      await saveStack(right.slice(0, pos + endPos), stack)

      return false
    }

    return false
  } else if (node instanceof BranchNode) {
    // Unset all internal nodes in the forkpoint
    for (let i = left[pos] + 1; i < right[pos]; i++) {
      node.setBranch(i, null)
    }

    {
      /**
       * `stack` records the path from root to fork point.
       * Since we need to unset both left and right nodes once,
       * we need to make a copy here.
       */
      const _stack = [...stack]
      const next = node.getBranch(left[pos])
      const child = next && (await trie.lookupNode(next))
      const endPos = await unset(trie, node, child, left.slice(pos), 1, false, _stack)
      await saveStack(left.slice(0, pos + endPos), _stack)
    }

    {
      const _stack = [...stack]
      const next = node.getBranch(right[pos])
      const child = next && (await trie.lookupNode(next))
      const endPos = await unset(trie, node, child, right.slice(pos), 1, true, _stack)
      await saveStack(right.slice(0, pos + endPos), _stack)
    }

    return false
  } else {
    throw new Error('invalid node')
  }
}

/**
 * Verifies a proof and return the verified trie.
 * @param rootHash - root hash.
 * @param key - target key.
 * @param proof - proof node list.
 * @throws If proof is found to be invalid.
 * @returns The value from the key, or null if valid proof of non-existence.
 */
async function verifyProof(
  rootHash: Buffer,
  key: Buffer,
  proof: Buffer[],
  useKeyHashingFunction: HashKeysFunction
): Promise<{ value: Buffer | null; trie: Trie }> {
  const proofTrie = new Trie({ root: rootHash, useKeyHashingFunction })
  try {
    await proofTrie.fromProof(proof)
  } catch (e) {
    throw new Error('Invalid proof nodes given')
  }
  try {
    const value = await proofTrie.get(key, true)
    return {
      trie: proofTrie,
      value,
    }
  } catch (err: any) {
    if (err.message === 'Missing node in DB') {
      throw new Error('Invalid proof provided')
    } else {
      throw err
    }
  }
}

/**
 * hasRightElement returns the indicator whether there exists more elements
 * on the right side of the given path
 * @param trie - trie object.
 * @param key - given path.
 */
async function hasRightElement(trie: Trie, key: Nibbles): Promise<boolean> {
  let pos = 0
  let node = await trie.lookupNode(trie.root())
  while (node !== null) {
    if (node instanceof BranchNode) {
      for (let i = key[pos] + 1; i < 16; i++) {
        if (node.getBranch(i) !== null) {
          return true
        }
      }

      const next = node.getBranch(key[pos])
      node = next && (await trie.lookupNode(next))
      pos += 1
    } else if (node instanceof ExtensionNode) {
      if (
        key.length - pos < node.keyLength() ||
        nibblesCompare(node._nibbles, key.slice(pos, pos + node.keyLength())) !== 0
      ) {
        return nibblesCompare(node._nibbles, key.slice(pos)) > 0
      }

      pos += node.keyLength()
      node = await trie.lookupNode(node._value)
    } else if (node instanceof LeafNode) {
      return false
    } else {
      throw new Error('invalid node')
    }
  }
  return false
}

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
export async function verifyRangeProof(
  rootHash: Buffer,
  firstKey: Nibbles | null,
  lastKey: Nibbles | null,
  keys: Nibbles[],
  values: Buffer[],
  proof: Buffer[] | null,
  useKeyHashingFunction: HashKeysFunction
): Promise<boolean> {
  if (keys.length !== values.length) {
    throw new Error('invalid keys length or values length')
  }

  // Make sure the keys are in order
  for (let i = 0; i < keys.length - 1; i++) {
    if (nibblesCompare(keys[i], keys[i + 1]) >= 0) {
      throw new Error('invalid keys order')
    }
  }
  // Make sure all values are present
  for (const value of values) {
    if (value.length === 0) {
      throw new Error('invalid values')
    }
  }

  // All elements proof
  if (proof === null && firstKey === null && lastKey === null) {
    const trie = new Trie({ useKeyHashingFunction })
    for (let i = 0; i < keys.length; i++) {
      await trie.put(nibblesToBuffer(keys[i]), values[i])
    }
    if (rootHash.compare(trie.root()) !== 0) {
      throw new Error('invalid all elements proof: root mismatch')
    }
    return false
  }

  if (proof === null || firstKey === null || lastKey === null) {
    throw new Error(
      'invalid all elements proof: proof, firstKey, lastKey must be null at the same time'
    )
  }

  // Zero element proof
  if (keys.length === 0) {
    const { trie, value } = await verifyProof(
      rootHash,
      nibblesToBuffer(firstKey),
      proof,
      useKeyHashingFunction
    )

    if (value !== null || (await hasRightElement(trie, firstKey))) {
      throw new Error('invalid zero element proof: value mismatch')
    }

    return false
  }

  // One element proof
  if (keys.length === 1 && nibblesCompare(firstKey, lastKey) === 0) {
    const { trie, value } = await verifyProof(
      rootHash,
      nibblesToBuffer(firstKey),
      proof,
      useKeyHashingFunction
    )

    if (nibblesCompare(firstKey, keys[0]) !== 0) {
      throw new Error('invalid one element proof: firstKey should be equal to keys[0]')
    }
    if (value === null || value.compare(values[0]) !== 0) {
      throw new Error('invalid one element proof: value mismatch')
    }

    return hasRightElement(trie, firstKey)
  }

  // Two edge elements proof
  if (nibblesCompare(firstKey, lastKey) >= 0) {
    throw new Error('invalid two edge elements proof: firstKey should be less than lastKey')
  }
  if (firstKey.length !== lastKey.length) {
    throw new Error(
      'invalid two edge elements proof: the length of firstKey should be equal to the length of lastKey'
    )
  }

  const trie = new Trie({ root: rootHash, useKeyHashingFunction })
  await trie.fromProof(proof)

  // Remove all nodes between two edge proofs
  const empty = await unsetInternal(trie, firstKey, lastKey)
  if (empty) {
    trie.root(trie.EMPTY_TRIE_ROOT)
  }

  // Put all elements to the trie
  for (let i = 0; i < keys.length; i++) {
    await trie.put(nibblesToBuffer(keys[i]), values[i])
  }

  // Compare rootHash
  if (trie.root().compare(rootHash) !== 0) {
    throw new Error('invalid two edge elements proof: root mismatch')
  }

  return hasRightElement(trie, keys[keys.length - 1])
}
