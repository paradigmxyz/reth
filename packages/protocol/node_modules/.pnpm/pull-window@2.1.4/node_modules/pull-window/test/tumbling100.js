var pull   = require('pull-stream')
var window = require('../')
var test = require('tape')

var expected = 
[ { start: 0,
    data: [ 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14 ] },
  { start: 15, data: [ 15, 16, 17, 18, 19, 20 ] },
  { start: 21, data: [ 21, 22, 23, 24, 25 ] },
  { start: 26, data: [ 26, 27, 28, 29 ] },
  { start: 30, data: [ 30, 31, 32, 33 ] },
  { start: 34, data: [ 34, 35, 36 ] },
  { start: 37, data: [ 37, 38, 39 ] },
  { start: 40, data: [ 40, 41, 42 ] },
  { start: 43, data: [ 43, 44, 45 ] },
  { start: 46, data: [ 46, 47, 48 ] },
  { start: 49, data: [ 49, 50, 51 ] },
  { start: 52, data: [ 52, 53 ] },
  { start: 54, data: [ 54, 55 ] },
  { start: 56, data: [ 56, 57 ] },
  { start: 58, data: [ 58, 59 ] },
  { start: 60, data: [ 60, 61 ] },
  { start: 62, data: [ 62, 63 ] },
  { start: 64, data: [ 64, 65 ] },
  { start: 66, data: [ 66, 67 ] },
  { start: 68, data: [ 68, 69 ] },
  { start: 70, data: [ 70, 71 ] },
  { start: 72, data: [ 72, 73 ] },
  { start: 74, data: [ 74, 75 ] },
  { start: 76, data: [ 76, 77 ] },
  { start: 78, data: [ 78, 79 ] },
  { start: 80, data: [ 80, 81 ] },
  { start: 82, data: [ 82, 83 ] },
  { start: 84, data: [ 84, 85 ] },
  { start: 86, data: [ 86, 87 ] },
  { start: 88, data: [ 88, 89 ] },
  { start: 90, data: [ 90, 91 ] },
  { start: 92, data: [ 92, 93 ] },
  { start: 94, data: [ 94, 95 ] },
  { start: 96, data: [ 96, 97 ] },
  { start: 98, data: [ 98, 99 ] },
  { start: 100, data: [ 100 ] }
]

function groupTo100 () {
  var sum = []
  return window(function (_, cb) {
    if(sum.length) return
    //if you don't want to start a window here,
    //return undefined

    //else return a function.
    //this will be called all data
    //until you callback.
    return function (end, data) {
      if(end) return cb(null, sum)
      sum.push(data)
      var total = sum.reduce(function (a, b) { return a + b }, 0)
      if(total >= 100) {
        var _sum = sum
        sum = []
        cb(null, _sum)
      }
    }
  })
}

test('tumbling to 100', function (t) {

  pull(
    pull.count(100),
    groupTo100(),
    pull.collect(function (err, ary) {
      t.deepEqual(ary, expected)
      console.log(ary)
      t.end()
    })
  )

})

