const CheckpointTrie = require('./index')
const secureInterface = require('./secure-interface')
const inherits = require('util').inherits

module.exports = SecureTrie
inherits(SecureTrie, CheckpointTrie)

/**
 * You can create a secure Trie where the keys are automatically hashed using **SHA3** by using `require('merkle-patricia-tree/secure')`. It has the same methods and constuctor as `Trie`
 * @class SecureTrie
 * @extends Trie
 */
function SecureTrie () {
  CheckpointTrie.apply(this, arguments)
  secureInterface(this)
}
