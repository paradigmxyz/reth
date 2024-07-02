import type { Nibbles } from '../types'

/**
 * Prepends hex prefix to an array of nibbles.
 * @param key - Array of nibbles
 * @returns returns buffer of encoded data
 **/
export function addHexPrefix(key: Nibbles, terminator: boolean): Nibbles {
  // odd
  if (key.length % 2) {
    key.unshift(1)
  } else {
    // even
    key.unshift(0)
    key.unshift(0)
  }

  if (terminator) {
    key[0] += 2
  }

  return key
}

/**
 * Removes hex prefix of an array of nibbles.
 * @param val - Array of nibbles
 * @private
 */
export function removeHexPrefix(val: Nibbles): Nibbles {
  if (val[0] % 2) {
    val = val.slice(1)
  } else {
    val = val.slice(2)
  }

  return val
}

/**
 * Returns true if hex-prefixed path is for a terminating (leaf) node.
 * @param key - a hex-prefixed array of nibbles
 * @private
 */
export function isTerminator(key: Nibbles): boolean {
  return key[0] > 1
}
