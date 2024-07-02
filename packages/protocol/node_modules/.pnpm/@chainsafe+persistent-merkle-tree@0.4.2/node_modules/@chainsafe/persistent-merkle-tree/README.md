# Persistent Merkle Tree

![ES Version](https://img.shields.io/badge/ES-2020-yellow)
![Node Version](https://img.shields.io/badge/node-12.x-green)

A binary merkle tree implemented as a [persistent data structure](https://en.wikipedia.org/wiki/Persistent_data_structure).

## Example

```typescript

// LeafNode and BranchNode are used to build nodes in a tree
// Nodes may not be changed once initialized

import {LeafNode, BranchNode} from "@chainsafe/persistent-merkle-tree";

const leaf = LeafNode.fromRoot(Buffer.alloc(32, 0xaa));
const otherLeaf = LeafNode.fromRoot(Buffer.alloc(32, 0xbb));

const branch = new BranchNode(leaf, otherLeaf);

// The `root` property returns the merkle root of a Node

const r: Uint8Array = branch.root; // == hash(leaf.root, otherLeaf.root));

// The `isLeaf` method returns true if the Node is a LeafNode

branch.isLeaf() === false;
leaf.isLeaf() === true;

// Well-known zero nodes are provided

import {zeroNode} from "@chainsafe/persistent-merkle-tree";

// 0x0
const zero0 = zeroNode(0);

// hash(0, 0)
const zero1 = zeroNode(1);

// hash(hash(0, 0), hash(0, 0))
const zero1 = zeroNode(2);

// Tree provides a mutable wrapper around a "root" Node

import {Tree} from "@chainsafe/persistent-merkle-tree";

const tree = new Tree(zeroNode(10));

// `rootNode` property returns the root Node of a Tree

const rootNode: Node = tree.rootNode;

// `root` property returns the merkle root of a Tree

const rr: Uint8Array = tree.root;

// A Tree is navigated by Gindex

const gindex = BigInt(...);

const n: Node = tree.getNode(gindex); // the Node at gindex
const rrr: Uint8Array = tree.getRoot(gindex); // the Uint8Array root at gindex
const subtree: Tree = tree.getSubtree(gindex); // the Tree wrapping the Node at gindex. Updates to `subtree` will be propagated to `tree`

// A merkle proof for a gindex can be generated

const proof: Uint8Array[] = tree.getSingleProof(gindex);

// Multiple types of proofs are supported through the `getProof` interface

// For example, a multiproof for multiple gindices can be generated like so

import {ProofType} from "@chainsafe/persistent-merkle-tree";

const gindices: BigInt[] = [...];

const proof: Proof = tree.getProof({
  type: ProofType.treeOffset,
  gindices,
});

// `Proof` objects can be used to recreate `Tree` objects
// These `Tree` objects can be navigated as usual for all nodes contained in the proof
// Navigating to unknown/unproven nodes results in an error

const partialTree: Tree = Tree.createFromProof(proof);

const unknownGindex: BigInt = ...;

gindices.includes(unknownGindex) // false

partialTree.getRoot(unknownGindex) // throws

```

## Motivation

When dealing with large datasets, it is very expensive to merkleize them in their entirety. In cases where large datasets are remerkleized often between updates and additions, using ephemeral structures for intermediate hashes results in significant duplicated work, as many intermediate hashes will be recomputed and thrown away on each merkleization. In these cases, maintaining structures for the entire tree, intermediate nodes included, can mitigate these issues and allow for additional usecases (eg: proof generation). This implementation also uses the known immutability of nodes to share data between common subtrees across different versions of the data.

## Features

#### Intermediate nodes with cached, lazily computed hashes

The tree is represented as a linked tree of `Node`s, currently either `BranchNode`s or `LeafNode`s.
A `BranchNode` has a `left` and `right` child `Node`, and a `root`, 32 byte `Uint8Array`.
A `LeafNode` has a `root`.
The `root` of a `Node` is not computed until requested, and cached thereafter.

#### Shared data betwen common subtrees

Any update to a tree (either to a leaf or intermediate node) is performed as a rebinding that yields a new, updated tree that maximally shares data between versions. Garbage collection allows memory from unused nodes to be eventually reclaimed.

#### Mutable wrapper for the persistent core

A `Tree` object wraps `Node` and provides an API for tree navigation and transparent rebinding on updates.

#### Navigation by gindex or (depth, index)

Many tree methods allow navigation with a gindex. A gindex (or generalized index) describes a path through the tree, starting from the root and nagivating downwards.

```
     1
   /   \
  2     3
/  \   /  \
4  5   6  7
```

It can also be interpreted as a bitstring, starting with "1", then appending "0" for each navigation left, or "1" for each navigation right.

```
        1
    /      \
   10       11
 /    \    /    \
100  101  110  111
```

Alternatively, several tree methods, with names ending with `AtDepth`, allow navigation by (depth, index). Depth and index navigation works by first navigating down levels into the tree from the top, starting at 0 (depth), and indexing nodes from the left, starting at 0 (index).

```
     0          <- depth 0
   /   \
  0     1       <- depth 1
/  \   /  \
0  1   2  3     <- depth 2
```

#### Memory efficiency

As an implementation detail, nodes hold their values as a `HashObject` (a javascript object with `h0`, ... `h7` uint32 `number` values) internally, rather than as a 32 byte `Uint8Array`. Memory benchmarking shows that this results in a ~3x reduction in memory over `Uint8Array`. This optimization allows this library to practically scale to trees with millions of nodes.

#### Navigation efficiency

In performance-critical applications performing many reads and writes to trees, being smart with tree navigation is crucial. This library correctly provides tree navigation methods that handle several important optimized cases: multi-node get and set, and get-then-set operations.

## See also:

https://github.com/protolambda/remerkleable

### Audit

This repo was audited by Least Authority as part of [this security audit](https://github.com/ChainSafe/lodestar/blob/master/audits/2020-03-23_UTILITY_LIBRARIES.pdf), released 2020-03-23. Commit [`8b5ad7`](https://github.com/ChainSafe/bls-hd-key/commit/8b5ad7) verified in the report.

## License

Apache-2.0
