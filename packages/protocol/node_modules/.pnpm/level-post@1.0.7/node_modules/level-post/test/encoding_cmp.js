var level = require('level-test')()
var post = require('../')
var test = require('tape')
var bytewise = require('bytewise')

test('encoding lt', function (t) {

  var data = [
    { type: 'put', key: [ 'z', 1 ], value: 'a' },
    { type: 'put', key: [ 'z', 2 ], value: 'b' },
    { type: 'put', key: [ 'q', 3 ], value: 'c' },
    { type: 'put', key: [ 'zz', 4 ], value: 'd' },
    { type: 'put', key: [ 'a', 5 ], value: 'e' },
    { type: 'put', key: [ 'z', 6 ], value: 'f' }
  ]
  var expected = [ 'b' ]
  t.plan(expected.length * 3)

  var db = level('encoding lt', { keyEncoding: bytewise })

  var opts = {
    lt: [ 'z', 4 ],
    gt: [ 'z', 1 ]
  }
  post(db, opts, function (op) {
    t.ok(op.key)
    t.equal(op.value, expected.shift())
    t.equal(op.type, 'put')
  })
  db.batch(data)
})

test('encoding gte', function (t) {

  var data = [
    { type: 'put', key: [ 'z', 1 ], value: 'a' },
    { type: 'put', key: [ 'z', 2 ], value: 'b' },
    { type: 'put', key: [ 'q', 3 ], value: 'c' },
    { type: 'put', key: [ 'zz', 4 ], value: 'd' },
    { type: 'put', key: [ 'a', 5 ], value: 'e' },
    { type: 'put', key: [ 'z', 6 ], value: 'f' }
  ]
  var expected = [ 'b', 'f' ]
  t.plan(expected.length * 3)

  var db = level('encoding gte', { keyEncoding: bytewise })

  var opts = {
    lt: [ 'z', undefined ],
    gte: [ 'z', 2 ]
  }
  post(db, opts, function (op) {
    t.ok(op.key)
    t.equal(op.value, expected.shift())
    t.equal(op.type, 'put')
  })
  db.batch(data)
})

