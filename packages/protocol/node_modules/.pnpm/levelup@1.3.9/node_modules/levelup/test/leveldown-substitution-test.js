/* Copyright (c) 2012-2016 LevelUP contributors
 * See list at <https://github.com/level/levelup#contributing>
 * MIT License <https://github.com/level/levelup/blob/master/LICENSE.md>
 */

var levelup = require('../lib/levelup.js')
var assert = require('referee').assert
var refute = require('referee').refute
var buster = require('bustermove')
var MemDOWN = require('memdown')

require('./common')

buster.testCase('LevelDOWN Substitution', {
  'test substitution of LevelDOWN with MemDOWN': function (done) {
    var md = new MemDOWN('foo')
    var db = levelup('/somewhere/not/writable/booya!', { db: function () { return md } })
    var entries = []
    var expected = [
      { key: 'a', value: 'A' },
      { key: 'b', value: 'B' },
      { key: 'c', value: 'C' },
      { key: 'd', value: 'D' },
      { key: 'e', value: 'E' },
      { key: 'f', value: 'F' },
      { key: 'i', value: 'I' }
    ]

    db.put('f', 'F')
    db.put('h', 'H')
    db.put('i', 'I')
    db.put('a', 'A')
    db.put('c', 'C')
    db.put('e', 'E')
    db.del('g')
    db.batch([
      { type: 'put', key: 'd', value: 'D' },
      { type: 'del', key: 'h' },
      { type: 'put', key: 'b', value: 'B' }
    ])

    db.createReadStream()
      .on('data', function (data) { entries.push(data) })
      .on('error', function (err) { refute(err, 'readStream emitted an error') })
      .on('close', function () {
        assert.equals(entries, expected, 'correct entries')
        assert.equals(
          md._store['$foo'].keys,
          expected.map(function (e) { return e.key }),
          'memdown has the entries'
        )
        done()
      })
  }
})
