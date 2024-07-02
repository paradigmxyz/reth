

var tape = require('tape')

var sublevel = require('../')
var level = require('level-test')()

tape('inherit json encoding', function (t) {

  var db = sublevel(level('simple', {valueEncoding: 'json'}))
  db.put('hello', {ok: true}, function (err) {
    if(err) throw err

    db.get('hello', function (err, value) {
      if(err) throw err

      t.deepEqual(value, {ok: true})
      var db2 = db.sublevel('sub')
      db2.put('hello', {ok: true}, function (err) {
        if(err) throw err

        db2.get('hello', function (err, value) {
          if(err) throw err
          t.deepEqual(value, {ok: true})
          t.end()
        })
      })
    })
  })
})

tape('override json encoding', function (t) {
  var db = sublevel(level('level-sublevel_override', {valueEncoding: 'json'}))
  var buf = new Buffer([1,2,3,4])

  db.put('hello', buf, function (err) {
    if(err) throw err

    db.get('hello', function (err, value) {
      if(err) throw err

      t.deepEqual(value.data || value, [].slice.call(buf))
      var db2 = db.sublevel('sub', {valueEncoding: 'binary'})
      db2.put('hello', buf, function (err) {
        if(err) throw err

        db2.get('hello', function (err, value) {
          if(err) throw err
          t.deepEqual(value, buf)
          t.end()
        })
      })
    })
  })
})
