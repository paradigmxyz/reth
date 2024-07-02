const Trie = require('../index.js')
const tape = require('tape')
const crypto = require('crypto')

tape('put & get raw functions', function (it) {
  var trie = new Trie()
  var key = crypto.randomBytes(32)
  var val = crypto.randomBytes(32)

  it.test('putRaw', function (t) {
    trie.putRaw(key, val, t.end)
  })

  it.test('getRaw', function (t) {
    trie.getRaw(key, function (err, rVal) {
      t.equal(val.toString('hex'), rVal.toString('hex'))
      t.end(err)
    })
  })

  it.test('should checkpoint and get the rawVal', function (t) {
    trie.checkpoint()
    trie.getRaw(key, function (err, rVal) {
      t.equal(val.toString('hex'), rVal.toString('hex'))
      t.end(err)
    })
  })

  var key2 = crypto.randomBytes(32)
  var val2 = crypto.randomBytes(32)
  it.test('should store while in a checkpoint', function (t) {
    trie.putRaw(key2, val2, t.end)
  })

  it.test('should retrieve from a checkpoint', function (t) {
    trie.getRaw(key2, function (err, rVal) {
      t.equal(val2.toString('hex'), rVal.toString('hex'))
      t.end(err)
    })
  })

  it.test('should not retiev after revert', function (t) {
    trie.revert(t.end)
  })

  it.test('should delete raw', function (t) {
    trie.delRaw(val2, t.end)
  })

  it.test('should not get val after delete ', function (t) {
    trie.getRaw(val2, function (err, val) {
      t.notok(val)
      t.end(err)
    })
  })

  var key3 = crypto.randomBytes(32)
  var val3 = crypto.randomBytes(32)

  it.test('test commit behavoir', function (t) {
    trie.checkpoint()
    trie.putRaw(key3, val3, function () {
      trie.commit(function () {
        trie.getRaw(key3, function (err, val) {
          t.equal(val.toString('hex'), val3.toString('hex'))
          t.end(err)
        })
      })
    })
  })
})
