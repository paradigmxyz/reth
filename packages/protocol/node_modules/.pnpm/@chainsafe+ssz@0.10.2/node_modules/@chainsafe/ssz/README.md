# ssz

![ES Version](https://img.shields.io/badge/ES-2020-yellow)
![Node Version](https://img.shields.io/badge/node-12.x-green)

## Summary

[Simple Serialize (SSZ)](https://github.com/ethereum/consensus-specs/blob/dev/ssz/simple-serialize.md) is a consensus layer standard that defines how consensus objects are serialized and merkleized.

SSZ is a type system that defines:

- efficient serialization / deserialization
- stable merkleization
- default constructor

Additionally, this library allows for additional operations:

- create and consume proofs
- to / from json-serializable object
- copy / clone

## Install

`npm install @chainsafe/ssz`

## Usage

```typescript
import {ContainerType, ByteVectorType} from "@chainsafe/ssz";

// Creates a "Keypair" SSZ data type (a private key of 32 bytes, a public key of 48 bytes)
const Keypair = new ContainerType({
    privateKey: new ByteVectorType(32),
    publicKey: new ByteVectorType(48),
  },
});

// The type value of any SSZ types are derived with `ValueOf`
import {ValueOf} from "@chainsafe/ssz";
type Keypair = ValueOf<typeof Keypair>

// Now you can perform different operations on Keypair objects
const keypair = Keypair.defaultValue(); // Create a default Keypair

keypair.privateKey; // => Uint8Array [0,0,0,...], length 32
keypair.publicKey; // => Uint8Array [0,0,0, ...], length 48

// serialize the object to a byte array
const serialized: Uint8Array = Keypair.serialize(keypair);

// get the merkle root of the object
const root: Uint8Array = Keypair.hashTreeRoot(keypair);

// create a copy of the object
const keypair2: Keypair = Keypair.clone(kp);

// deserialize a serialized object
const keypair3: Keypair = Keypair.deserialize(serialized);

// Convert to JSON-serializable representation
// (binary data is converted to hex strings)
const keypairJSON = Keypair.toJson(keypair);
const keypairJSONStr = JSON.stringify(jsonKp);

// convert the json-serializable representation to the object
const keypair4: Keypair = Keypair.fromJson(keypairJSON);

// The merkle-tree-backed representation of a Keypair may be created / operated on
const keypairView: TreeBacked<Keypair> = Keypair.toView(keypair)

// All of the same operations can be performed on tree-backed values
keypairView.serialize();
```

### Ethereum consensus objects

For Ethereum consensus datatypes (eg: `BeaconBlock`, `DepositData`, `BeaconState`, etc), see [`@chainsafe/lodestar-types`](https://github.com/ChainSafe/lodestar/tree/master/packages/lodestar-types).

## Additional notes

### Backings

This library operates on values of two kinds of 'backings', or underlying representations of data. Each backing has runtime tradeoffs for the above operations that arise from the nature of the underlying representation.

Prior versions of this library attempted to fully align the interfaces between operations on various backings. This has the side effect of obscuring the underlying representation in downstream code, which is undesirable when maintaining code that requires higher levels control, eg: performance-critical code. This library no longer supports interchanging values with different backings, and exported APIs clearly distinguish between backings.

We support the following backings:

- Value - This backing has a native javascript type representation.

Containers are constructed as Javascript Objects, vectors and lists as Arrays (or TypedArrays). Type methods `type.serialize`. `type.deserialize`, `type.hashTreeRoot`, and `type.defaultValue` all operate on values.

- Tree - This backing has an immutable merkle tree representation.

The data is represented as a full merkle tree, composed of immutable, linked nodes. (See [`@chainsafe/persistent-merkle-tree`](https://github.com/ChainSafe/ssz/tree/master/packages/persistent-merkle-tree)), wrapped as a "tree view". Two types of tree view are provided, a simple wrapper, and a wrapper with more caching and batched updates.

### Tree View

A tree view is a wrapper around a `Tree` and a `Type` that provides methods for convenient property access and ssz operations.

Property getters return sub-views, except for basic types, which return native values. Setters, likewise, require sub-views, except for basic types, which require native views.

This tree view is a simple wrapper to tree backed data that commits any changes immediately to the tree. Changes are propagated upwards to the root parent tree.

```ts
// Create a type
const C = new ContainerType({
  a: new VectorBasicType(new UintNumberType(1), 2),
});

// Create a tree view based on the default value
const c = C.defaultView();

// SSZ operations
c.serialize() === C.hashTreeRoot(C.defaultValue());
const root = c.hashTreeRoot();

// Getters
c.a.get(0) === 0;

// Setters
// Changes are applied immediately to the tree
c.a.set(0, 1);

// Subsequent calls to `hashTreeRoot` reflect the changes to the tree
assert(root.toString() !== c.hashTreeRoot().toString());
```

If you need to do many mutations at once see **ViewDU**, which defers all updates to a later `commit` step, paying the cost of updating the tree only once.

**Subview behaviour**

View implementations don't contain any internal caches beyond their internal `Tree`s, and setting one subview to another will not link the views.

```ts
const c1 = C.toView({a: [0, 0]});
const c2 = C.toView({a: [1, 1]});

// c1's Tree now includes the root node of `c2.a` but no references to `c2.a` view
// Warning: this is different behaviour than ViewDU
c1.a = c2.a;

// This statement mutates ONLY c1 data
c1.a.set(0, 2);
// This statement mutates ONLY c2 data
c2.a.set(0, 3);
```

### Tree ViewDU

ViewDU = View Deferred Update. This tree view caches all mutations to data and applies the changes to the tree only when requested by calling the `commit` method. This allows to pay to cost of navigating and updating the tree only once. This strategy is optimal for large tree manipulations that require very high performance (i.e. the Ethereum consensus beacon chain state transition).

```ts
// Create a type
const C = new ContainerType({
  a: new VectorBasicType(new UintNumberType(1), 2),
});

// Create a tree view DU based on the default value
const c = C.defaultViewDU();

// SSZ operations
c.serialize() === C.hashTreeRoot(C.defaultValue());
const root = c.hashTreeRoot();

// Getters
c.a.get(0) === 0;

// Setters
// Changes are NOT applied immediately to the tree
c.a.set(0, 1);

// Subsequent calls to `hashTreeRoot` do NOT reflect the changes to the tree
assert(root.toString() === c.hashTreeRoot().toString());

// Until commit is called
c.commit();

assert(root.toString() !== c.hashTreeRoot().toString());
```

**Key features**

- Defer tree updates until `commit` is called, allowing multiple nodes to tree to be set in a batch and navigating through the tree at most once
- Persist caches of sub-properties to prevent tree navigation when re-reading data.

**Subview behaviour**

Due to having mutable caches for children properties, setting a subview from one view to the subview of another view will link the two by referencing the same underlying cache.

```ts
const c1 = C.toViewDU({a: [0, 0]});
const c2 = C.toViewDU({a: [1, 1]});

// Now both c1 and c2 have a reference to the exact same cached child view
// Warning: this is different behaviour than View
c1.a = c2.a;

// This statement mutates c1 AND c2 data
c1.a.set(0, 2);
// This statement mutates c1 AND c2 data
c2.a.set(0, 3);
```

## License

Apache 2.0
