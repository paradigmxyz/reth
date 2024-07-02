import { addHexPrefix } from '../../util/hex'

import { Node } from './node'

import type { Nibbles } from '../../types'

export class ExtensionNode extends Node {
  constructor(nibbles: Nibbles, value: Buffer) {
    super(nibbles, value, false)
  }

  static encodeKey(key: Nibbles): Nibbles {
    return addHexPrefix(key, false)
  }
}
