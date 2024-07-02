/* Copyright (c) 2012-2016 LevelUP contributors
 * See list at <https://github.com/level/levelup#contributing>
 * MIT License <https://github.com/level/levelup/blob/master/LICENSE.md>
 */

var common = require('./common')
var assert = require('referee').assert
var buster = require('bustermove')

buster.testCase('Argument checking', {
  'setUp': common.commonSetUp,
  'tearDown': common.commonTearDown,

  'test get() throwables': function (done) {
    this.openTestDatabase(function (db) {
      assert.exception(
        db.get.bind(db),
        { name: 'ReadError', message: 'get() requires key and callback arguments' },
        'no-arg get() throws'
      )

      assert.exception(
        db.get.bind(db, 'foo'),
        { name: 'ReadError', message: 'get() requires key and callback arguments' },
        'callback-less, 1-arg get() throws'
      )

      assert.exception(
        db.get.bind(db, 'foo', {}),
        { name: 'ReadError', message: 'get() requires key and callback arguments' },
        'callback-less, 2-arg get() throws'
      )

      done()
    })
  },

  'test put() throwables': function (done) {
    this.openTestDatabase(function (db) {
      assert.exception(
        db.put.bind(db),
        { name: 'WriteError', message: 'put() requires a key argument' },
        'no-arg put() throws'
      )

      done()
    })
  },

  'test del() throwables': function (done) {
    this.openTestDatabase(function (db) {
      assert.exception(
        db.del.bind(db),
        { name: 'WriteError', message: 'del() requires a key argument' },
        'no-arg del() throws'
      )

      done()
    })
  },

  'test approximateSize() throwables': function (done) {
    this.openTestDatabase(function (db) {
      assert.exception(
        db.approximateSize.bind(db),
        { name: 'ReadError', message: 'approximateSize() requires start, end and callback arguments' },
        'no-arg approximateSize() throws'
      )

      assert.exception(
        db.approximateSize.bind(db, 'foo'),
        { name: 'ReadError', message: 'approximateSize() requires start, end and callback arguments' },
        'callback-less, 1-arg approximateSize() throws'
      )

      assert.exception(
        db.approximateSize.bind(db, 'foo', 'bar'),
        { name: 'ReadError', message: 'approximateSize() requires start, end and callback arguments' },
        'callback-less, 2-arg approximateSize() throws'
      )

      assert.exception(
        db.approximateSize.bind(db, 'foo', 'bar', {}),
        { name: 'ReadError', message: 'approximateSize() requires start, end and callback arguments' },
        'callback-less, 3-arg approximateSize(), no cb throws'
      )

      done()
    })
  },

  'test batch() throwables': function (done) {
    this.openTestDatabase(function (db) {
      assert.exception(
        db.batch.bind(db, null, {}),
        { name: 'WriteError', message: 'batch() requires an array argument' },
        'no-arg batch() throws'
      )

      assert.exception(
        db.batch.bind(db, {}),
        { name: 'WriteError', message: 'batch() requires an array argument' },
        '1-arg, no Array batch() throws'
      )

      done()
    })
  }
})
