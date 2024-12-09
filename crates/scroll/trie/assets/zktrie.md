# zkTrie Spec

## 1. Tree Structure

<figure>
<img src="https://raw.githubusercontent.com/scroll-tech/reth/refs/heads/scroll/crates/scroll/trie/assets/arch.png" alt="zkTrie Structure" style="width:80%">
<figcaption align = "center"><b>Figure 1. zkTrie Structure</b></figcaption>
</figure>

In essence, zkTrie is a sparse binary Merkle Patricia Trie, depicted in the above figure.
Before diving into the Sparse Binary Merkle Patricia Trie, let's briefly touch on Merkle Trees and Patricia Tries.
* **Merkle Tree**: A Merkle Tree is a tree where each leaf node represents a hash of a data block, and each non-leaf node represents the hash of its child nodes.
* **Patricia Trie**: A Patricia Trie is a type of radix tree or compressed trie used to store key-value pairs efficiently. It encodes the nodes with same prefix of the key to share the common path, where the path is determined by the value of the node key.

As illustrated in the Figure 1, there are three types of nodes in the zkTrie.
- Parent Node (type: 0): Given the zkTrie is a binary tree, a parent node has two children.
- Leaf Node (type: 1): A leaf node holds the data of a key-value pair.
- Empty Node (type: 2): An empty node is a special type of node, indicating the sub-trie that shares the same prefix is empty.

In zkTrie, we use Poseidon hash to compute the node hash because it's more friendly and efficient to prove it in the zk circuit.

## 2. Tree Construction

Given a key-value pair, we first compute a *secure key* for the corresponding leaf node by hashing the original key (i.e., account address and storage key) using the Poseidon hash function. This can make the key uniformly distributed over the key space. The node key hashing method is described in the [Node Hashing](#3-node-hashing) section below.

We then encode the path of a new leaf node by traversing the secure key from Least Significant Bit (LSB) to the Most Significant Bit (MSB). At each step, if the bit is 0, we will traverse to the left child; otherwise, traverse to the right child.

We limit the maximum depth of zkTrie to 248, meaning that the tree will only traverse the lower 248 bits of the key. This is because the secure key space is a finite field used by Poseidon hash that doesn't occupy the full range of power of 2. This leads to an ambiguous bit representation of the key in a finite field and thus causes a soundness issue in the zk circuit. But if we truncate the key to lower 248 bits, the key space can fully occupy the range of $2^{248}$ and won't have the ambiguity in the bit representation.

We also apply an optimization to reduce the tree depth by contracting a subtree that has only one leaf node to a single leaf node. For example, in the Figure 1, the tree has three nodes in total, with keys `0100`, `0010`, and `1010`. Because there is only one node that has key with suffix `00`, the leaf node for key `0100` only traverses the suffix `00` and doesn't fully expand its key which would have resulted in depth of 4.

## 3. Node Hashing

In this section, we will describe how leaf secure key and node merkle hash are computed. We use Poseidon hash in both hashing computation, denoted as `h` in the doc below.

<aside>
💡 Note: We use `init_state = 0` in the Poseidon hash function for all use cases in the zkTrie.
</aside>

### 3.1 Empty Node

The node hash of an empty node is 0.

### 3.2 Parent Node

The parent node hash is computed as follows

```go
parentNodeHash = h(leftChildHash, rightChildHash)
```

### 3.3 Leaf Node

The node hash of a leaf node is computed as follows

```go
leafNodeHash = h(h(1, nodeKey), valueHash)
```

The leaf node can hold two types of values: Ethereum accounts and storage key-value pairs. Next, we will describe how the node key and value hash are computed for each leaf node type.

#### Ethereum Account Leaf Node
For an Ethereum Account Leaf Node, it consists of an Ethereum address and a state account struct. The secure key is derived from the Ethereum address.
```
address[0:20] (20 bytes in big-endian)
valHi = address[0:16]
valLo = address[16:20] * 2^96 (padding 12 bytes of 0 at the end)
nodeKey = h(valHi, valLo)
```

A state account struct in the Scroll consists of the following fields (`Fr` indicates the finite field used in Poseidon hash and is a 254-bit value)

- `Nonce`: u64
- `Balance`: u256, but treated as Fr
- `StorageRoot`: Fr
- `KeccakCodeHash`: u256
- `PoseidonCodeHash`: Fr
- `CodeSize`: u64

Before computing the value hash, the state account is first marshaled into a list of `u256` values. The marshaling scheme is

```
(The following scheme assumes the big-endian encoding)
[0:32] (bytes in big-endian)
	[0:16] Reserved with all 0
	[16:24] CodeSize, uint64 in big-endian
	[24:32] Nonce, uint64 in big-endian
[32:64] Balance
[64:96] StorageRoot
[96:128] KeccakCodeHash
[128:160] PoseidonCodehash
(total 160 bytes)
```

The marshal function also returns a `flag` value along with a vector of `u256` values. The `flag` is a bitmap that indicates whether a `u256` value CANNOT be treated as a field element (Fr). The `flag` value for state account is 8, shown below.

```
+--------------------+---------+------+----------+----------+
|          0         |    1    |   2  |     3    |     4    | (index)
+--------------------+---------+------+----------+----------+
| nonce||codesize||0 | balance | root |  keccak  | poseidon | (u256)
+--------------------+---------+------+----------+----------+
|          0         |    0    |   0  |     1    |     0    | (flag bits)
+--------------------+---------+------+----------+----------+
(LSB)                                                   (MSB)
```

The value hash is computed in two steps:
1. Convert the value that cannot be represented as a field element of the Poseidon hash to the field element.
2. Combine field elements in a binary tree structure till the tree root is treated as the value hash.

In the first step, when the bit in the `flag` is 1 indicating the `u256` value that cannot be treated as a field element, we split the value into a high-128bit value and a low-128bit value, and then pass them to a Poseidon hash to derive a field element value, `h(valueHi, valueLo)`.

Based on the definition, the value hash of the state account is computed as follows.

```
valueHash =
h(
    h(
        h(nonce||codesize||0, balance),
        h(
            storageRoot,
            h(keccakCodeHash[0:16], keccakCodeHash[16:32]), // convert Keccak codehash to a field element
        ),
    ),
    poseidonCodeHash,
)
```

#### Storage Leaf Node

For a Storage Leaf Node, it is a key-value pair, which both are a `u256` value. The secure key of this leaf node is derived from the storage key.

```
storageKey[0:32] (32 bytes in big-endian)
valHi = storageKey[0:16]
valLo = storageKey[16:32]
nodeKey = h(valHi, valLo)
```

The storage value is a `u256` value. The `flag` for the storage value is 1, showed below.

```
+--------------+
|      0       | (index)
+--------------+
| storageValue | (u256)
+--------------+
|      1       | (flag bits)
+--------------+
```

The value hash is computed as follows

```go
valueHash = h(storageValue[0:16], storageValue[16:32])
```

## 4. Tree Operations

### 4.1 Insertion

<figure>
<img src="https://raw.githubusercontent.com/scroll-tech/reth/refs/heads/scroll/crates/scroll/trie/assets/insertion.png" alt="zkTrie Structure" style="width:80%">
<figcaption align = "center"><b>Figure 2. Insert a new leaf node to zkTrie</b></figcaption>
</figure>

When we insert a new leaf node to the existing zkTrie, there could be two cases illustrated in the Figure 2.

1. When traversing the path of the node key, it reaches an empty node (Figure 2(b)). In this case, we just need to replace this empty node by this leaf node and backtrace the path to update the merkle hash of parent nodes till the root.
2. When traversing the path of the node key, it reaches another leaf node `b` (Figure 2(c)). In this case, we need to push down the existing leaf node `b` until the next bit in the node keys of two leaf nodes differs. At each push-down step, we need to insert an empty sibling node when necessary. When we reach the level where the bits differ, we then place two leaf nodes `b` and `c` as the left child and the right child depending on their bits. At last, we backtrace the path and update the merkle hash of all parent nodes.

### 4.2 Deletion

<figure>
<img src="https://raw.githubusercontent.com/scroll-tech/reth/refs/heads/scroll/crates/scroll/trie/assets/deletion.png" alt="zkTrie Structure" style="width:80%">
<figcaption align = "center"><b>Figure 3. Delete a leaf node from the zkTrie</b></figcaption>
</figure>


The deletion of a leaf node is similar to the insertion. There are two cases illustrated in the Figure 3.

1. The sibling node of to-be-deleted leaf node is a parent node (Figure 3(b)). In this case, we can just replace the node `a` by an empty node and update the node hash of its ancestors till the root node.
2. The node of to-be-deleted leaf node is a leaf node (Figure 3(c)). Similarly, we first replace the leaf node by an empty node and start to contract its sibling node upwards until its sibling node is not an empty node. For example, in Figure 3(c), we first replace the leaf node `b` by an empty node. During the contraction, since the sibling of node `c` now becomes an empty node, we move node `c` one level upward to replace its parent node. The new sibling of node `c`, node `e`, is still an empty node. So again we move node `c` upward. Now that the sibling of node `c` is node `a`, the deletion process is finished.

Note that the sibling of a leaf node in a valid zkTrie cannot be an empty node. Otherwise, we should always prune the subtree and move the leaf node upwards.
