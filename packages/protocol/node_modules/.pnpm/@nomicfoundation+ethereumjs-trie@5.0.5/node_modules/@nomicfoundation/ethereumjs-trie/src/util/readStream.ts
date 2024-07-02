import { Readable } from 'readable-stream'

import { BranchNode, LeafNode } from '../trie'

import { nibblesToBuffer } from './nibbles'

import type { Trie } from '../trie'
import type { FoundNodeFunction } from '../types'

export class TrieReadStream extends Readable {
  private trie: Trie
  private _started: boolean

  constructor(trie: Trie) {
    super({ objectMode: true })

    this.trie = trie
    this._started = false
  }

  async _read() {
    if (this._started) {
      return
    }
    this._started = true
    try {
      await this._findValueNodes(async (_, node, key, walkController) => {
        if (node !== null) {
          this.push({
            key: nibblesToBuffer(key),
            value: node.value(),
          })
          walkController.allChildren(node, key)
        }
      })
    } catch (error: any) {
      if (error.message === 'Missing node in DB') {
        // pass
      } else {
        throw error
      }
    }
    this.push(null)
  }

  /**
   * Finds all nodes that store k,v values
   * called by {@link TrieReadStream}
   * @private
   */
  async _findValueNodes(onFound: FoundNodeFunction): Promise<void> {
    const outerOnFound: FoundNodeFunction = async (nodeRef, node, key, walkController) => {
      let fullKey = key

      if (node instanceof LeafNode) {
        fullKey = key.concat(node.key())
        // found leaf node!
        onFound(nodeRef, node, fullKey, walkController)
      } else if (node instanceof BranchNode && node.value()) {
        // found branch with value
        onFound(nodeRef, node, fullKey, walkController)
      } else {
        // keep looking for value nodes
        if (node !== null) {
          walkController.allChildren(node, key)
        }
      }
    }
    await this.trie.walkTrie(this.trie.root(), outerOnFound)
  }
}
