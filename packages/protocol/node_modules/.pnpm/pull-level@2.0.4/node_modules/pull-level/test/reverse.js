var levelup  = require('level')
var SubLevel = require('level-sublevel')
var pull     = require('pull-stream')

var path    = '/tmp/pull-level-read-reverse'
require('rimraf').sync(path)
var db      = SubLevel(levelup(path))

var l = require('../')
var all = []

var h = require('./helper')

var test = require('tape')

function filter(range) {
  require('tape')('ranges:' + JSON.stringify(range), 
  function (t) {

    pull(
      l.read(db, {reverse: true}),
      pull.collect(function (err, ary) {
        t.notOk(err)
        t.equal(ary.length, all.length)
        t.deepEqual(h.sort(ary), all.filter(function (e) {
          return (
            (!range.min || range.min <= e.key) && 
            (!range.max || range.max >= e.key)
          )
        }))
        t.end()
      })
    )
  })
}

require('tape')('reverse', function (t) {

  var second = false

  h.words(db, function (err, all) {

    t.notOk(err)
    var i = 0
    pull(
      l.read(db, {reverse: true}),
      pull.collect(function (err, ary) {
        t.notOk(err)
        t.equal(ary.length, all.length)
        t.deepEqual(ary, all.reverse())
        t.end()
      })
    )
  })
})

