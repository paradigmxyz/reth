
var test = require('tape')
var pull = require('../')

test('filtered randomnes', function (t) {
  pull(
    pull.infinite(),
    pull.filter(function (d) {
      if (process.env.TEST_VERBOSE) console.log('f', d)
      return d > 0.5
    }),
    pull.take(100),
    pull.collect(function (err, array) {
      t.equal(array.length, 100)
      array.forEach(function (d) {
        t.ok(d > 0.5)
        t.ok(d <= 1)
      })
      if (process.env.TEST_VERBOSE) console.log(array)
      t.end()
    })
  )
})

test('filter with regexp', function (t) {
  pull(
    pull.infinite(),
    pull.map(function (d) {
      return Math.round(d * 1000).toString(16)
    }),
    pull.filter(/^[^e]+$/i), //no E
    pull.take(37),
    pull.collect(function (err, array) {
      t.equal(array.length, 37)
      if (process.env.TEST_VERBOSE) console.log(array)
      array.forEach(function (d) {
        t.equal(d.indexOf('e'), -1)
      })
      t.end()
    })
  )
})

test('inverse filter with regexp', function (t) {
  pull(
    pull.infinite(),
    pull.map(function (d) {
      return Math.round(d * 1000).toString(16)
    }),
    pull.filterNot(/^[^e]+$/i), //no E
    pull.take(37),
    pull.collect(function (err, array) {
      t.equal(array.length, 37)
      array.forEach(function (d) {
        t.notEqual(d.indexOf('e'), -1)
      })
      t.end()
    })
  )
})

