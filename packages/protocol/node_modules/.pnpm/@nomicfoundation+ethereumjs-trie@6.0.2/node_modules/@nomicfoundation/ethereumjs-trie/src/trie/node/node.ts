import { RLP } from '@nomicfoundation/ethereumjs-rlp'
import { bufArrToArr } from '@nomicfoundation/ethereumjs-util'

import { addHexPrefix, removeHexPrefix } from '../../util/hex'
import { nibblesToBuffer } from '../../util/nibbles'

import type { Nibbles } from '../../types'

export class Node {
  _nibbles: Nibbles
  _value: Buffer
  _terminator: boolean

  constructor(nibbles: Nibbles, value: Buffer, terminator: boolean) {
    this._nibbles = nibbles
    this._value = value
    this._terminator = terminator
  }

  static decodeKey(key: Nibbles): Nibbles {
    return removeHexPrefix(key)
  }

  key(k?: Nibbles): Nibbles {
    if (k !== undefined) {
      this._nibbles = k
    }

    return this._nibbles.slice(0)
  }

  keyLength() {
    return this._nibbles.length
  }

  value(v?: Buffer) {
    if (v !== undefined) {
      this._value = v
    }

    return this._value
  }

  encodedKey(): Nibbles {
    return addHexPrefix(this._nibbles.slice(0), this._terminator)
  }

  raw(): [Buffer, Buffer] {
    return [nibblesToBuffer(this.encodedKey()), this._value]
  }

  serialize(): Buffer {
    return Buffer.from(RLP.encode(bufArrToArr(this.raw())))
  }
}
