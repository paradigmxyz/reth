var level = require('level-test')()
var post = require('../')
var test = require('tape')

test('batch', function (t) {

  var n = 10

  var db = level('simple')

  post(db, function (op) {
    t.ok(op.key)
    t.ok(op.value)
    t.equal(op.type, 'put')
    t.ok(n--)

    if(!n)
      t.end()
  })

  function op () {
    return { type: 'put', key: Math.random(), value: new Date() }
  }

  var m = n/2

  while(m--)
    db.batch([op(), op()], function (err) {
      if(err) throw err
    })
})

