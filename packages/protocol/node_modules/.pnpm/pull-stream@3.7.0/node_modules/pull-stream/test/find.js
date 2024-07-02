
var pull = require('../')
var test = require('tape')

test('find 7', function (t) {
  pull(
    pull.values([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]),
    pull.find(function (d) {
      return d == 7
    }, function (err, seven) {
      t.equal(seven, 7)
      t.notOk(err)
      t.end()
    })
  )
})

var target = Math.random()
test('find ' + target, function (t) {
  var f = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10].map(Math.random)

  f.push(target)
  pull(
    pull.values(f.sort()),
    pull.find(function (d) {
      return d == target
    }, function (err, found) {
      t.equal(found, target)
      t.notOk(err)
      t.end()
    })
  )
})

test('find missing', function (t) {
  var f = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]

  pull(
    pull.values(f.sort()),
    pull.find(function (d) {
      return d == target
    }, function (err, found) {
      t.equal(found, null)
      t.notOk(err)
      t.end()
    })
  )
})


test('there can only be one', function (t) {

  pull(
    pull.values([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]),
    pull.asyncMap(function (e, cb) {
      process.nextTick(function () {
        cb(null, e)
      })
    }),
    pull.find(function (d) {
      return d >= 7
    }, function (err, seven) {
      t.equal(seven, 7)
      t.notOk(err)
      t.end()
    })
  )

})

test('find null', function (t) {
  pull(
    pull.values([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]),
    pull.find(null, function (err, first) {
      t.equal(first, 1)
      t.notOk(err)
      t.end()
    })
  )
})