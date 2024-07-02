import { addHexPrefix } from '../../util/hex'

import { Node } from './node'

import type { Nibbles } from '../../types'

export class LeafNode extends Node {
  constructor(nibbles: Nibbles, value: Buffer) {
    super(nibbles, value, true)
  }

  static encodeKey(key: Nibbles): Nibbles {
    return addHexPrefix(key, true)
  }
}
