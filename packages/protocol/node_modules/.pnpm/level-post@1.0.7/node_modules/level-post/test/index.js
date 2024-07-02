
var level = require('level-test')()
var post = require('../')
var test = require('tape')

test('simple', function (t) {

  var n = 5

  var db = level('simple')

  post(db, function (op) {
    t.ok(op.key)
    t.ok(op.value)
    t.ok(op.type)
    t.ok(n--)

    if(!n)
      t.end()
  })

  var m = n

  while(m--)
    db.put(Math.random(), new Date(), function (err) {
      if(err) throw err
    })
})
