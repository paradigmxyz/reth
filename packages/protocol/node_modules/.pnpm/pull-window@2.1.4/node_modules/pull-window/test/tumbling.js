
var pull = require('pull-stream')
var windows = require('../')

var test = require('tape')

function rememberStart (start, data) {
  return {start: start, data: data}
}

test('tumbling count', function (t) {

  var i = 0, last
  var expected = 
  [ { start: 0, data: 78 },
    { start: 13, data: 247 },
    { start: 26, data: 416 },
    { start: 39, data: 585 },
    { start: 52, data: 754 },
    { start: 65, data: 923 },
    { start: 78, data: 1092 },
    { start: 91, data: 1261 },
    { start: 104, data: 1430 } ]

  pull(
    pull.count(127),
    windows(function (data, cb) {
      if(!(i++ % 13)) {
        if(last) last()
        var acc = 0
        last = function () { cb(null, acc) }
        return function (end, data) {
          if(end) return
          acc = acc + data
        }
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

process.on('uncaughtException', function (err) {
  console.error(err.stack)
})


