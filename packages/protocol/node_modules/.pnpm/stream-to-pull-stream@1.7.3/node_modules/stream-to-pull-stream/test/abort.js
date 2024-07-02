var pull    = require('pull-stream')
var through = require('through')
var toPull  = require('../')
var Hang = require('pull-hang')
var Cat = require('pull-cat')
var tape = require('tape')

tape('abort', function (t) {

  t.plan(2)

  var ts = through()
  ts.on('close', function () {
    t.ok(true)
  })
  pull(
    pull.values([.1, .4, .6, 0.7, .94, 0.3]),
//  pull.infinite()
    toPull(ts),
    function (read) {
      console.log('reader!')
      read(null, function next (end, data) {
        console.log('>>>', end, data) 
        if(data > 0.9) {
          console.log('ABORT')
          read(true, function (end) {
            t.ok(true)
            t.end()
          })
        } else {
          read(null, next)
        }
      })
    }
  )
})

tape('abort hang', function (t) {
  var ts = through(), aborted = false, c = 0, _read, ended, closed
  ts.on('close', function () {
    closed = true
  })
  pull(
    Cat([
      pull.values([.1, .4, .6, 0.7, 0.3]),
      Hang(function () {
        aborted = true
      })
    ]),
    toPull(ts),
    function (read) {
      _read = read
      read(null, function next (end, data) {
        if(end) {
          ended = true
        }
        else read(null, next)
      })
    }
  )

  setTimeout(function () {
    _read(true, function (end) {
      t.ok(aborted, 'aborted')
      t.ok(ended, 'ended')
      t.ok(closed, 'closed')
      t.ok(end, 'abort cb end')
      t.end()
    })
  }, 10)
})



tape('abort a stream that has already ended', function (t) {

  var ts = through()

  var n = 0
  pull(
    toPull.source(ts),
    //like pull.take(4), but abort async.
    function (read) {
      return function (abort, cb) {
        console.log(n)
        if(n++ < 4) read(null, cb)
        else {
          //this would be quite a badly behaved node stream
          //but it can be difficult to make a node stream that behaves well.
          ts.end()
          setTimeout(function () {
            read(true, cb)
          }, 10)
        }
      }
    },
    pull.collect(function (err, ary) {
      if(err) throw err
      t.deepEqual(ary, [1,2,3,4])
      t.end()
    })
  )

  ts.queue(1)
  ts.queue(2)
  ts.queue(3)
  ts.queue(4)

})













