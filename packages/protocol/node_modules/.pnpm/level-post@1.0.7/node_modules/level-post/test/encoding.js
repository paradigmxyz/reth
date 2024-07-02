var level = require('level-test')()
var post = require('../')
var test = require('tape')
var bytewise = require('bytewise')


var data = [
  { type: 'put', key: [ 'z', 1 ], value: 'a' },
  { type: 'put', key: [ 'z', 2 ], value: 'b' },
  { type: 'put', key: [ 'q', 3 ], value: 'c' },
  { type: 'put', key: [ 'zz', 4 ], value: 'd' },
  { type: 'put', key: [ 'a', 5 ], value: 'e' },
  { type: 'put', key: [ 'z', 6 ], value: 'f' }
]

var n = 0
function testBounds (opts) {
  var db = level('encoding-'+Date.now(), { keyEncoding: bytewise })

  test('encoding', function (t) {

    var expected = [ 'a', 'b', 'f' ]
    t.plan(expected.length * 3)

    post(db, opts, function (op) {
      t.ok(op.key)
      t.equal(op.value, expected.shift())
      t.equal(op.type, 'put')
      console.log(op)
    })
    db.batch(data)
  })

}

testBounds({
  start: [ 'z', null ],
  end: [ 'z', undefined ]
})
testBounds({
  gte: [ 'z', null ],
  lte: [ 'z', undefined ]
})
testBounds({
  gt: [ 'z', null ],
  lt: [ 'z', undefined ]
})

