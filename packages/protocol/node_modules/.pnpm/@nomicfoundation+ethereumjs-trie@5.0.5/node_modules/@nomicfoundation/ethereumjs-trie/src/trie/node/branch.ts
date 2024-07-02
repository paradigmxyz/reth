import { RLP } from '@nomicfoundation/ethereumjs-rlp'
import { bufArrToArr } from '@nomicfoundation/ethereumjs-util'

import type { EmbeddedNode } from '../../types'

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

  value(v?: Buffer | null): Buffer | null {
    if (v !== null && v !== undefined) {
      this._value = v
    }

    return this._value && this._value.length > 0 ? this._value : null
  }

  setBranch(i: number, v: EmbeddedNode | null) {
    this._branches[i] = v
  }

  raw(): (EmbeddedNode | null)[] {
    return [...this._branches, this._value]
  }

  serialize(): Buffer {
    return Buffer.from(RLP.encode(bufArrToArr(this.raw() as Buffer[])))
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
