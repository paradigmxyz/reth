/* Copyright (c) 2012-2016 LevelUP contributors
 * See list at <https://github.com/level/levelup#contributing>
 * MIT License <https://github.com/level/levelup/blob/master/LICENSE.md>
 */

var levelup = require('../lib/levelup.js')
var async = require('async')
var common = require('./common')

var assert = require('referee').assert
var refute = require('referee').refute
var buster = require('bustermove')

buster.testCase('approximateSize()', {
  'setUp': common.commonSetUp,
  'tearDown': common.commonTearDown,

  'approximateSize() is deprecated': function (done) {
    this.openTestDatabase(function (db) {
      var error = console.error
      console.error = function (str) {
        console.error = error
        assert(/deprecated/.test(str))
        done()
      }
      db.approximateSize('a', 'z', function () {})
    })
  },

  'approximateSize() works on empty database': function (done) {
    this.openTestDatabase(function (db) {
      db.approximateSize('a', 'z', function (err, size) {
        refute(err) // sanity
        assert.equals(size, 0)
        done()
      })
    })
  },

  'approximateSize() work on none-empty database': function (done) {
    var location = common.nextLocation()
    var db

    async.series(
      [
        function (callback) {
          this.openTestDatabase(
                    location
                  , function (_db) {
                    db = _db
                    callback()
                  }
                )
        }.bind(this),
        function (callback) {
          var batch = []
          var i = 0

          for (; i < 10; ++i) {
            batch.push({ type: 'put', key: String(i), value: 'afoovalue' })
          }
          db.batch(batch, { sync: true }, callback)
        },
        function (callback) {
          // close db to make sure stuff gets written to disc
          db.close(callback)
        },
        function (callback) {
          levelup(location, function (err, _db) {
            refute(err)
            db = _db
            callback()
          })
        },
        function (callback) {
          db.approximateSize('0', '99', function (err, size) {
            refute(err) // sanity
            refute.equals(size, 0)
            callback()
          })
        }
      ], done)
  }
})
