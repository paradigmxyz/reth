
var levelup  = require('level')
//var SubLevel = require('level-sublevel')
var pull     = require('pull-stream')

var path    = '/tmp/pull-level-read-live'
require('rimraf').sync(path)
var db      = levelup(path)

var l = require('../')
var all = []

var h = require('./helper')

require('tape')('live', function (t) {

  var second = false, sync, ary, err = new Error('intentional')

  var n = 3

  h.random(db, 11, function (err2, all) {
    if(err2) throw err2

    pull(
      l.read(db, {live: true}),
      pull.through(function (data) {
        console.log("DATA", data)

      }, function (end) {
        console.log("END", end)
        t.ok(end)
        if(--n) return
        end()
      }),
      pull.filter(function (d) {
        if(d.sync) return !(sync = true)
        return true
      }),
      h.exactly(20, err),
      pull.map(function (e) {
        return {key: e.key, value: e.value}//drop 'type' from live updates
      }),
      pull.collect(function (err, _ary) {
        t.ok(err)

        ary = _ary
        if(--n) return
        end()
      })
    )

    h.random(db, 9, function (err, _all) {
      all = h.sort(all.concat(_all))
      if(--n) return
      end()
    })

    function end() {
      console.log(ary)
      console.log(all)
      console.log(ary.length, all.length)
      t.deepEqual(h.sort(ary), all)
      t.equal(ary.length, all.length)
      t.ok(sync)
      t.end()
    
    }
  })
}) 





