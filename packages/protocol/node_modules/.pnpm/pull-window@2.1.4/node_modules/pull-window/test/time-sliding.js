
var pull = require('pull-stream')
var windows = require('../')

var test = require('tape')

function rememberStart (start, data) {
  return {start: start, data: data}
}

test('timed window', function (t) {

  var timer
  var expected = 
  [ { start: 0, data: 3 },
    { start: 3, data: 12 },
    { start: 6, data: 21 },
    { start: 9, data: 30 } ]

  pull(
    pull.count(13),
    pull.asyncMap(function (data, cb) {
      setTimeout(function () {
        cb(null, data)
      }, 100)
    }),
    windows(function (data, cb) {
      if(timer) return
      var acc = 0
      timer = setTimeout(function () {
        timer = null
        console.log('acc', acc)
        cb(null, acc)
      }, 300)
      return function (end, data) {
        if(end) return
        acc += data
      }
    }, rememberStart),
    pull.collect(function (err, ary) {
      t.notOk(err)
      console.log(ary)
      t.deepEqual(ary, expected)
      t.end()      
    })
  )
})
