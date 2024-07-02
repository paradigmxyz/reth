

var tape = require('tape')
var pull = require('../')

function hang (values, onAbort) {
  var i = 0
  return function (abort, cb) {
    if(i < values.length)
      cb(null, values[i++])
    else if(!abort)
      _cb = cb
    else {
      _cb(abort)
      cb(abort) //??
      onAbort && onAbort()
    }
  }
}

function abortable () {
  var _read, aborted
  function reader (read) {
    _read = read
    return function (abort, cb) {
      if(abort) aborted = abort
      read(abort, cb)
    }
  }

  reader.abort = function (cb) {
    cb = cb || function (err) {
      if(err && err !== true) throw err
    }
    if(aborted)
      cb(aborted)
    else _read(true, cb)
  }

  return reader
}

function test (name, trx) {
  tape('test abort:'+name, function (t) {
    var a = abortable()

    pull(
      hang([1,2,3], function () {
        t.end()
      }),
      trx,
      a,
      pull.drain(function (e) {
        if(e === 3)
          setImmediate(function () {
            a.abort()
          })
      }, function (err) {
      })
    )
  })
}

test('through', pull.through())
test('map', pull.map(function (e) { return e }))
test('take', pull.take(Boolean))

