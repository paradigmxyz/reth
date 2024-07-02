var tape = require('tape');

var sublevel = require('../');
var level = require('level-test')()

tape('can call isOpen & isClosed', function (t) {
  var _db = level('level-sublevel-is-open')
  var db = sublevel(_db)

  t.equal(db.isOpen(), false)
  t.equal(db.isClosed(), false)

  _db.once('open', function () {
    t.equal(db.isOpen(), true)
    t.equal(db.isClosed(), false)

    _db.close(function () {
      t.equal(db.isOpen(), false)
      t.equal(db.isClosed(), true)

      t.end()
    })
  })
})
