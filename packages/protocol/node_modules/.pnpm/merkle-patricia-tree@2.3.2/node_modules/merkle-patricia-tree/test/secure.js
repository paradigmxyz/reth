const Trie = require('../secure.js')
const async = require('async')
const tape = require('tape')
const jsonTests = require('ethereumjs-testing').tests.trieTests.trietest_secureTrie

var trie = new Trie()

tape('secure tests', function (it) {
  it.test('empty values', function (t) {
    async.eachSeries(jsonTests.emptyValues.in, function (row, cb) {
      trie.put(new Buffer(row[0]), row[1], cb)
    }, function (err) {
      t.equal('0x' + trie.root.toString('hex'), jsonTests.emptyValues.root)
      t.end(err)
    })
  })

  it.test('branchingTests', function (t) {
    trie = new Trie()
    async.eachSeries(jsonTests.branchingTests.in, function (row, cb) {
      trie.put(row[0], row[1], cb)
    }, function () {
      t.equal('0x' + trie.root.toString('hex'), jsonTests.branchingTests.root)
      t.end()
    })
  })

  it.test('jeff', function (t) {
    async.eachSeries(jsonTests.jeff.in, function (row, cb) {
      var val = row[1]
      if (val) {
        val = new Buffer(row[1].slice(2), 'hex')
      }

      trie.put(new Buffer(row[0].slice(2), 'hex'), val, cb)
    }, function () {
      t.equal('0x' + trie.root.toString('hex'), jsonTests.jeff.root)
      t.end()
    })
  })
})
