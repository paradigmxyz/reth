var tape = require('tape');

var sublevel = require('../');
var level = require('level-test')()

tape('can call close on root sublevel', function (t) {
  var _db = level('level-sublevel-root-close')
  var db = sublevel(_db)

  _db.once('open', function () {
    db.close(function (err) {
      t.error(err)

      t.ok(_db.isClosed())
      t.end()
    })
  })
})

tape('can call close on sub sublevel', function (t) {
  var _db = level('level-sublevel-sub-close')
  var db = sublevel(_db)

  _db.once('open', function () {
    db.sublevel('foo').close(function (err) {
      t.error(err)

      t.notOk(_db.isClosed())
      t.end()
    })
  })
})
