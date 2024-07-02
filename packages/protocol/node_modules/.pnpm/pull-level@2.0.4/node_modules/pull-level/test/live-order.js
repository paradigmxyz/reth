
var levelup  = require('level')
//var SubLevel = require('level-sublevel')
var pull     = require('pull-stream')


var l = require('../')
var all = []

var h = require('./helper')
var tape = require('tape')

function toKV (d) { return {key: d.key, value: d.value}}

tape('live', function (t) {
  return t.end()
  var path    = '/tmp/pull-level-read-live'
  require('rimraf').sync(path)
  var db      = levelup(path)

  var second = false

  h.timestamps(db, 11, function (err, all) {

    var i = 0
    var sync = false
    pull(
      l.read(db, {live: true, values: false}),
      pull.filter(function (s) {
        if(s.sync) sync = true
        else return true
      }),
      h.exactly(20),
      pull.collect(function (err, ary) {
        process.nextTick(function () {
          t.notOk(err)
          t.ok(second)
          console.log("ALL", all)
          t.equal(ary.length, 20)
          t.equal(ary.length, all.length)
          t.deepEqual(ary, h.sort(ary.slice()))
          t.deepEqual(ary.map(toKV), all)
          t.ok(sync)
          t.end()
        })
      })
    )


    setTimeout(function () {
      h.timestamps(db, 9, function (err, _all) {
        second = true
        all = all.concat(_all)
        console.log('all', all, all.length)
      })
    }, 100)
  })
})

tape('live2', function (t) {

  var path    = '/tmp/pull-level-read-live2'
  require('rimraf').sync(path)
  var db      = levelup(path)

  var second = false

  var n = 2

  h.timestamps(db, 10, function (err, all) {

    var i = 0
    var sync = false
    pull(
      l.read(db, {live: true}),
      pull.filter(function (s) {
        if(s.sync) sync = true
        else return true
      }),
      h.exactly(20),
      pull.collect(function (err, _ary) {
        t.notOk(err)
        ary = _ary
        console.log('END1')
        if(--n) return
        end()
      })
    )


    setTimeout(function () {
      h.timestamps(db, 10, function (err, _all) {
        t.notOk(err)
        all = all.concat(_all)
        console.log('END2')
        if(--n) return
        end()
      })
    }, 100)

    function end () {
      console.log('END')
      t.equal(ary.length, 20)
      t.equal(ary.length, all.length)

      t.deepEqual(ary.map(toKV), all.map(toKV))
      t.ok(sync)
      t.end()

    }
  })
})

tape('live, sync:true', function (t) {
  var path    = '/tmp/pull-level-read-live3'
  require('rimraf').sync(path)
  var db      = levelup(path)

  var second = false, ary

  h.timestamps(db, 11, function (err, all) {

    var i = 0
    var sync = false
    pull(
      l.read(db, {live: true, sync: true}),
      pull.filter(function (data) {
        if(data.sync) sync = true
        else return true
      }),
      h.exactly(20),
      pull.collect(function (err, _ary) {
        t.notOk(err)
        t.notOk(second)
        ary = _ary
      })
    )


    setTimeout(function () {
      h.timestamps(db, 9, function (err, _all) {
        second = true
        all = all.concat(_all)
          console.log(ary)
          t.equal(ary.length, 20)
          t.equal(ary.length, all.length)

//          var values = all.map(toKV)

          t.deepEqual(ary.map(toKV), all)
          t.ok(sync)
          t.end()
      })
    }, 100)
  })

})








