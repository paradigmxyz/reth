
var pull = require('../')
var test = require('tape')

test('reduce becomes through', function (t) {
  pull(
    pull.values([1,2,3]),
    pull.reduce(function (a, b) {return a + b}, 0, function (err, val) {
      t.equal(val, 6)
      t.end()
    })
  )
})

test('reduce without initial value', function (t) {
  pull(
    pull.values([1,2,3]),
    pull.reduce(function (a, b) {return a + b}, function (err, val) {
      t.equal(val, 6)
      t.end()
    })
  )
})


test('reduce becomes drain', function (t) {
  pull(
    pull.values([1,2,3]),
    pull.reduce(
      function (a, b) {return a + b}, 
      0,
      function (err, acc) {
        t.equal(acc, 6)
        t.end()
      }
    )
  )
})


