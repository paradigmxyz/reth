/* Copyright (c) 2012-2016 LevelUP contributors
 * See list at <https://github.com/level/levelup#contributing>
 * MIT License <https://github.com/level/levelup/blob/master/LICENSE.md>
 */

var levelup = require('../lib/levelup.js')
var common = require('./common')
var assert = require('referee').assert
var refute = require('referee').refute
var buster = require('bustermove')

function test (fun) {
  return function (done) {
    var location = common.nextLocation()
    // 1) open database without callback, opens in worker thread
    var db = levelup(location, { createIfMissing: true, errorIfExists: true, valueEncoding: 'utf8' })

    this.closeableDatabases.push(db)
    this.cleanupDirs.push(location)
    assert.isObject(db)
    assert.equals(db.location, location)

    fun(db, done)
    // we should still be in a state of limbo down here, not opened or closed, but 'new'
    refute(db.isOpen())
    refute(db.isClosed())
  }
}

buster.testCase('Deferred open() is patch-safe', {
  'setUp': common.commonSetUp,
  'tearDown': common.commonTearDown,

  'put() on pre-opened database': test(function (db, done) {
    var put = db.put
    var called = 0

    db.put = function () {
      called++
      return put.apply(this, arguments)
    }

    db.put('key', 'VALUE', function () {
      assert.equals(called, 1)
      done()
    })
  }),
  'del() on pre-opened database': test(function (db, done) {
    var del = db.del
    var called = 0

    db.del = function () {
      called++
      return del.apply(this, arguments)
    }

    db.del('key', function () {
      assert.equals(called, 1)
      done()
    })
  }),
  'batch() on pre-opened database': test(function (db, done) {
    var batch = db.batch
    var called = 0

    db.batch = function () {
      called++
      return batch.apply(this, arguments)
    }

    db.batch([
      { key: 'key', value: 'v', type: 'put' },
      { key: 'key2', value: 'v2', type: 'put' }
    ], function () {
      assert.equals(called, 1)
      done()
    })
  })
})
