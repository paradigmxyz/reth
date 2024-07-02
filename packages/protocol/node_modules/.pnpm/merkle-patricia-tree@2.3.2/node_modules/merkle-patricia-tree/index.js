const BaseTrie = require('./baseTrie')
const checkpointInterface = require('./checkpoint-interface')
const inherits = require('util').inherits
const proof = require('./proof.js')

module.exports = CheckpointTrie

inherits(CheckpointTrie, BaseTrie)

function CheckpointTrie () {
  BaseTrie.apply(this, arguments)
  checkpointInterface.call(this, this)
}

CheckpointTrie.prove = proof.prove
CheckpointTrie.verifyProof = proof.verifyProof
