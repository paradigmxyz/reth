
var levelup = require('level')
var pull    = require('pull-stream')

var path    = '/tmp/pull-level-read-stream'
require('rimraf').sync(path)
var db      = levelup(path)

var l = require('../')
var all = []
require('tape')('read-stream', function (t) {

  pull(
    pull.infinite(),
    pull.map(function (e) {
      return {
        key: e.toString(),
        value: new Date().toString()
      }
    }),
    pull.take(20),
    pull.through(function (e) {
      all.push({key:e.key, value: e.value})
    }),
    l.write(db, function (err) {
      pull(
        l.read(db),
        pull.collect(function (err, ary) {
          t.equal(ary.length, all.length)
          t.deepEqual(ary, all.sort(function (a, b) {
            return Number(a.key) - Number(b.key)
          }))
          t.end()
        })
      )
    })
  )
})
