const Trie = require('../index.js')
const tape = require('tape')
const trie = new Trie()
const trie2 = new Trie()

const hex = 'FF44A3B3'
tape('encoding hexprefixes ', function (t) {
  trie.put(new Buffer(hex, 'hex'), 'test', function () {
    trie2.put('0x' + hex, 'test', function () {
      t.equal(trie.root.toString('hex'), trie2.root.toString('hex'))
      t.end()
    })
  })
})
