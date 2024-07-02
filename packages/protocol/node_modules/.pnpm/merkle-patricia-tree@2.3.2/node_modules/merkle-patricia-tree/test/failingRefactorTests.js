const async = require('async')
const tape = require('tape')

const Trie = require('../secure.js')
const trie = new Trie()
const a = new Buffer('f8448080a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0a155280bc3c09fd31b0adebbdd4ef3d5128172c0d2008be964dc9e10e0f0fedf', 'hex')
const ak = new Buffer('095e7baea6a6c7c4c2dfeb977efac326af552d87', 'hex')

const b = new Buffer('f844802ea056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0db94dc4aab9b6a1a11956906ea34f3252f394576aece12199b23b269bb2738ab', 'hex')
const bk = new Buffer('945304eb96065b2a98b57a48a06ae28d285a71b5', 'hex')

const c = new Buffer('f84c80880de0b6b3a7640000a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470', 'hex')
const ck = new Buffer('a94f5374fce5edbc8e2a8697c15331677e6ebf0b', 'hex')
  // checkpoint
  // checkpoint
  // commit
const d = new Buffer('f8488084535500b1a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0a155280bc3c09fd31b0adebbdd4ef3d5128172c0d2008be964dc9e10e0f0fedf', 'hex')
const dk = new Buffer('095e7baea6a6c7c4c2dfeb977efac326af552d87', 'hex')

const e = new Buffer('f8478083010851a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0db94dc4aab9b6a1a11956906ea34f3252f394576aece12199b23b269bb2738ab', 'hex')
const ek = new Buffer('945304eb96065b2a98b57a48a06ae28d285a71b5', 'hex')

const f = new Buffer('f84c01880de0b6b3540df72ca056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470', 'hex')
const fk = new Buffer('a94f5374fce5edbc8e2a8697c15331677e6ebf0b', 'hex')

// commit
const g = new Buffer('f8488084535500b1a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0a155280bc3c09fd31b0adebbdd4ef3d5128172c0d2008be964dc9e10e0f0fedf', 'hex')
const gk = new Buffer('095e7baea6a6c7c4c2dfeb977efac326af552d87', 'hex')

tape('secure tests shouldnt crash ', function (t) {
  async.series([

    function (done) {
      console.log('done')
      trie.put(ak, a, done)
    },
    function (done) {
      trie.put(bk, b, done)
    },
    function (done) {
      trie.put(ck, c, done)
    },
    function (done) {
      trie.checkpoint()
      trie.checkpoint()
      done()
    },
    function (done) {
      trie.commit(done)
    },
    function (done) {
      trie.put(dk, d, done)
    },
    function (done) {
      trie.put(ek, e, done)
    },
    function (done) {
      trie.put(fk, f, done)
    },
    function (done) {
      trie.commit(done)
    },
    function (done) {
      trie.put(gk, g, done)
    }
  ], function () {
    t.end()
  })
})
