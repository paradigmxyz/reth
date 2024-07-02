
var pull = require('pull-stream')
var windows = require('../')

var test = require('tape')

process.on('uncaughtException', function (err) {
  console.error(err.stack)
})

test('sliding window', function (t) {
  var expected = 
[ { start: 0,
    data: [ 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13 ] },
  { start: 7,
    data: [ 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20 ] },
  { start: 14,
    data: [ 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27 ] },
  { start: 21,
    data: [ 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34 ] },
  { start: 28,
    data: [ 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41 ] },
  { start: 35,
    data: [ 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48 ] },
  { start: 42,
    data: [ 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55 ] },
  { start: 49,
    data: [ 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62 ] },
  { start: 56,
    data: [ 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69 ] },
  { start: 63,
    data: [ 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76 ] },
  { start: 70,
    data: [ 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83 ] },
  { start: 77,
    data: [ 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90 ] },
  { start: 84,
    data: [ 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97 ] },
  { start: 91,
    data:
      [ 91, 92, 93, 94, 95, 96, 97, 98, 99, 100, 101, 102, 103, 104 ] 
  },
  { start: 98,
    data: 
      [ 98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111 ]
  },
  { start: 105,
    data:
      [ 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116, 117, 118 ]
  },
  { start: 112,
    data:
      [ 112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124, 125 ]
  }
]

  var i = 0
  pull(
    pull.count(127),
    windows(function (data, cb) {

      if(!(i++ % 7)) {
        var acc = []
        var n = 0
        return function (end, data) {
          if(end) return
          acc.push(data)
          if(++n > 13)
            cb(null, acc)
        }
      }
    }),
    pull.collect(function (err, ary) {
      t.notOk(err)
      console.log(ary)
      t.deepEqual(ary, expected)
      t.end()
    })
  )

})


