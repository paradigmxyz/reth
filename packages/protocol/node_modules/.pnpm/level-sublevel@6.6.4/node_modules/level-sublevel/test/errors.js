var tape = require('tape')

var sublevel = require('../')
var level = require('level-test')()

tape('not found error', function (t) {

  var db = sublevel(level('level-sublevel-notfound'))

  db.get('foo', function (err, value) {
    t.ok(err)
    t.notOk(value)
    t.equal(err.name, 'NotFoundError')
    t.end()
  })

})
