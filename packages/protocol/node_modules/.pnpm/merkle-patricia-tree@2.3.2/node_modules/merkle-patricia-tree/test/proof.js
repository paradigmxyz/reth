const Trie = require('../index.js')
const async = require('async')
const tape = require('tape')

tape('simple merkle proofs generation and verification', function (tester) {
  var it = tester.test
  it('create a merkle proof and verify it', function (t) {
    var trie = new Trie()

    async.series([
      function (cb) {
        trie.put('key1aa', '0123456789012345678901234567890123456789xx', cb)
      },
      function (cb) {
        trie.put('key2bb', 'aval2', cb)
      },
      function (cb) {
        trie.put('key3cc', 'aval3', cb)
      },
      function (cb) {
        Trie.prove(trie, 'key2bb', function (err, prove) {
          if (err) return cb(err)
          Trie.verifyProof(trie.root, 'key2bb', prove, function (err, val) {
            if (err) return cb(err)
            t.equal(val.toString('utf8'), 'aval2')
            cb()
          })
        })
      },
      function (cb) {
        Trie.prove(trie, 'key1aa', function (err, prove) {
          if (err) return cb(err)
          Trie.verifyProof(trie.root, 'key1aa', prove, function (err, val) {
            if (err) return cb(err)
            t.equal(val.toString('utf8'), '0123456789012345678901234567890123456789xx')
            cb()
          })
        })
      },
      function (cb) {
        Trie.prove(trie, 'key2bb', function (err, prove) {
          if (err) return cb(err)
          Trie.verifyProof(trie.root, 'randomkey', prove, function (err, val) {
            t.notEqual(err, null, 'Expected error: ' + err.message)
            cb()
          })
        })
      },
      function (cb) {
        Trie.prove(trie, 'key2bb', function (err, prove) {
          if (err) return cb(err)
          Trie.verifyProof(trie.root, 'key2b', prove, function (err, val) {
            t.notEqual(err, null, 'Expected error: ' + err.message)
            cb()
          })
        })
      },
      function (cb) {
        Trie.prove(trie, 'key2bb', function (err, prove) {
          if (err) return cb(err)
          prove.push(Buffer.from('123456'))
          Trie.verifyProof(trie.root, 'key2b', prove, function (err, val) {
            t.notEqual(err, null, 'Expected error: ' + err.message)
            cb()
          })
        })
      }
    ], function (err) {
      t.end(err)
    })
  })

  it('create a merkle proof and verify it with a single long key', function (t) {
    var trie = new Trie()

    async.series([
      function (cb) {
        trie.put('key1aa', '0123456789012345678901234567890123456789xx', cb)
      },
      function (cb) {
        Trie.prove(trie, 'key1aa', function (err, prove) {
          if (err) return cb(err)
          Trie.verifyProof(trie.root, 'key1aa', prove, function (err, val) {
            if (err) return cb(err)
            t.equal(val.toString('utf8'), '0123456789012345678901234567890123456789xx')
            cb()
          })
        })
      }
    ], function (err) {
      t.end(err)
    })
  })

  it('create a merkle proof and verify it with a single short key', function (t) {
    var trie = new Trie()

    async.series([
      function (cb) {
        trie.put('key1aa', '01234', cb)
      },
      function (cb) {
        Trie.prove(trie, 'key1aa', function (err, prove) {
          if (err) return cb(err)
          Trie.verifyProof(trie.root, 'key1aa', prove, function (err, val) {
            if (err) return cb(err)
            t.equal(val.toString('utf8'), '01234')
            cb()
          })
        })
      }
    ], function (err) {
      t.end(err)
    })
  })

  it('create a merkle proof and verify it whit keys in the midle', function (t) {
    var trie = new Trie()

    async.series([
      function (cb) {
        trie.put('key1aa', '0123456789012345678901234567890123456789xxx', cb)
      },
      function (cb) {
        trie.put('key1', '0123456789012345678901234567890123456789Very_Long', cb)
      },
      function (cb) {
        trie.put('key2bb', 'aval3', cb)
      },
      function (cb) {
        trie.put('key2', 'short', cb)
      },
      function (cb) {
        trie.put('key3cc', 'aval3', cb)
      },
      function (cb) {
        trie.put('key3', '1234567890123456789012345678901', cb)
      },
      function (cb) {
        Trie.prove(trie, 'key1', function (err, prove) {
          if (err) return cb(err)
          Trie.verifyProof(trie.root, 'key1', prove, function (err, val) {
            if (err) return cb(err)
            t.equal(val.toString('utf8'), '0123456789012345678901234567890123456789Very_Long')
            cb()
          })
        })
      },
      function (cb) {
        Trie.prove(trie, 'key2', function (err, prove) {
          if (err) return cb(err)
          Trie.verifyProof(trie.root, 'key2', prove, function (err, val) {
            if (err) return cb(err)
            t.equal(val.toString('utf8'), 'short')
            cb()
          })
        })
      },
      function (cb) {
        Trie.prove(trie, 'key3', function (err, prove) {
          if (err) return cb(err)
          Trie.verifyProof(trie.root, 'key3', prove, function (err, val) {
            if (err) return cb(err)
            t.equal(val.toString('utf8'), '1234567890123456789012345678901')
            cb()
          })
        })
      }
    ], function (err) {
      t.end(err)
    })
  })

  it('should succeed with a simple embedded extension-branch', function (t) {
    var trie = new Trie()

    async.series([
      (cb) => {
        trie.put('a', 'a', cb)
      }, (cb) => {
        trie.put('b', 'b', cb)
      }, (cb) => {
        trie.put('c', 'c', cb)
      }, (cb) => {
        Trie.prove(trie, 'a', function (err, prove) {
          if (err) return cb(err)
          Trie.verifyProof(trie.root, 'a', prove, function (err, val) {
            if (err) return cb(err)
            t.equal(val.toString('utf8'), 'a')
            cb()
          })
        })
      }, (cb) => {
        Trie.prove(trie, 'b', function (err, prove) {
          if (err) return cb(err)
          Trie.verifyProof(trie.root, 'b', prove, function (err, val) {
            if (err) return cb(err)
            t.equal(val.toString('utf8'), 'b')
            cb()
          })
        })
      }, (cb) => {
        Trie.prove(trie, 'c', function (err, prove) {
          if (err) return cb(err)
          Trie.verifyProof(trie.root, 'c', prove, function (err, val) {
            if (err) return cb(err)
            t.equal(val.toString('utf8'), 'c')
            cb()
          })
        })
      }], function (err) {
      t.end(err)
    })
  })
})
