import { keccak256, rlp } from 'ethereumjs-util'
import { bufferToNibbles, nibblesToBuffer } from './util/nibbles'
import { isTerminator, addHexPrefix, removeHexPrefix } from './util/hex'

export type TrieNode = BranchNode | ExtensionNode | LeafNode
export type Nibbles = number[]
// Branch and extension nodes might store
// hash to next node, or embed it if its len < 32
export type EmbeddedNode = Buffer | Buffer[]

export class BranchNode {
  _branches: (EmbeddedNode | null)[]
  _value: Buffer | null

  constructor() {
    this._branches = new Array(16).fill(null)
    this._value = null
  }

  static fromArray(arr: Buffer[]): BranchNode {
    const node = new BranchNode()
    node._branches = arr.slice(0, 16)
    node._value = arr[16]
    return node
  }

  get value(): Buffer | null {
    return this._value && this._value.length > 0 ? this._value : null
  }

  set value(v: Buffer | null) {
    this._value = v
  }

  setBranch(i: number, v: EmbeddedNode | null) {
    this._branches[i] = v
  }

  raw(): (EmbeddedNode | null)[] {
    return [...this._branches, this._value]
  }

  serialize(): Buffer {
    return rlp.encode(this.raw())
  }

  hash(): Buffer {
    return keccak256(this.serialize())
  }

  getBranch(i: number) {
    const b = this._branches[i]
    if (b !== null && b.length > 0) {
      return b
    } else {
      return null
    }
  }

  getChildren(): [number, EmbeddedNode][] {
    const children: [number, EmbeddedNode][] = []
    for (let i = 0; i < 16; i++) {
      const b = this._branches[i]
      if (b !== null && b.length > 0) {
        children.push([i, b])
      }
    }
    return children
  }
}

export class ExtensionNode {
  _nibbles: Nibbles
  _value: Buffer

  constructor(nibbles: Nibbles, value: Buffer) {
    this._nibbles = nibbles
    this._value = value
  }

  static encodeKey(key: Nibbles): Nibbles {
    return addHexPrefix(key, false)
  }

  static decodeKey(key: Nibbles): Nibbles {
    return removeHexPrefix(key)
  }

  get key(): Nibbles {
    return this._nibbles.slice(0)
  }

  set key(k: Nibbles) {
    this._nibbles = k
  }

  get keyLength() {
    return this._nibbles.length
  }

  get value(): Buffer {
    return this._value
  }

  set value(v: Buffer) {
    this._value = v
  }

  encodedKey(): Nibbles {
    return ExtensionNode.encodeKey(this._nibbles.slice(0))
  }

  raw(): [Buffer, Buffer] {
    return [nibblesToBuffer(this.encodedKey()), this._value]
  }

  serialize(): Buffer {
    return rlp.encode(this.raw())
  }

  hash(): Buffer {
    return keccak256(this.serialize())
  }
}

export class LeafNode {
  _nibbles: Nibbles
  _value: Buffer

  constructor(nibbles: Nibbles, value: Buffer) {
    this._nibbles = nibbles
    this._value = value
  }

  static encodeKey(key: Nibbles): Nibbles {
    return addHexPrefix(key, true)
  }

  static decodeKey(encodedKey: Nibbles): Nibbles {
    return removeHexPrefix(encodedKey)
  }

  get key(): Nibbles {
    return this._nibbles.slice(0)
  }

  set key(k: Nibbles) {
    this._nibbles = k
  }

  get keyLength() {
    return this._nibbles.length
  }

  get value(): Buffer {
    return this._value
  }

  set value(v: Buffer) {
    this._value = v
  }

  encodedKey(): Nibbles {
    return LeafNode.encodeKey(this._nibbles.slice(0))
  }

  raw(): [Buffer, Buffer] {
    return [nibblesToBuffer(this.encodedKey()), this._value]
  }

  serialize(): Buffer {
    return rlp.encode(this.raw())
  }

  hash(): Buffer {
    return keccak256(this.serialize())
  }
}

export function decodeRawNode(raw: Buffer[]): TrieNode {
  if (raw.length === 17) {
    return BranchNode.fromArray(raw)
  } else if (raw.length === 2) {
    const nibbles = bufferToNibbles(raw[0])
    if (isTerminator(nibbles)) {
      return new LeafNode(LeafNode.decodeKey(nibbles), raw[1])
    }
    return new ExtensionNode(ExtensionNode.decodeKey(nibbles), raw[1])
  } else {
    throw new Error('Invalid node')
  }
}

export function decodeNode(raw: Buffer): TrieNode {
  const des = rlp.decode(raw)
  if (!Array.isArray(des)) {
    throw new Error('Invalid node')
  }
  return decodeRawNode(des)
}

export function isRawNode(n: any): boolean {
  return Array.isArray(n) && !Buffer.isBuffer(n)
}
