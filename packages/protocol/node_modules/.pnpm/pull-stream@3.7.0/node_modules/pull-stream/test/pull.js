var tape = require('tape')

function curry (fun) {
  return function () {
    var args = [].slice.call(arguments)
    return function (read) {
      return fun.apply(null, [read].concat(args))
    }
  }
}

var pull = require('../')

function values (array) {
  var i = 0
  return function (abort, cb) {
    if(abort) i = array.length, cb(abort)
    else if(i >= array.length) cb(true)
    else cb(null, array[i++])
  }
}

var map = curry(function (read, mapper) {
    return function (abort, cb) {
      read(abort, function (end, data) {
        if(end) cb(end)
        else cb(null, mapper(data))
      })
    }
  })

var sum = curry(function (read, done) {
    var total = 0
    read(null, function next (end, data) {
        if(end) return done(end === true ? null : end, total)
        total += data
        read(null, next)
      })
  })

var log = curry(function (read) {
    return function (abort, cb) {
      read(abort, function (end, data) {
        if(end) return cb(end)
        if (process.env.TEST_VERBOSE) console.error(data)
        cb(null, data)
      })
    }
  })

tape('wrap pull streams into stream', function (t) {

  pull(
    values([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]),
    map(function (e) { return e*e }),
    log(),
    sum(function (err, value) {
      if (process.env.TEST_VERBOSE) console.log(value)
      t.equal(value, 385)
      t.end()
    })
  )

})

tape('turn pull(through,...) -> Through', function (t) {

  pull(
    values([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]),
    pull(
      map(function (e) { return e*e }),
      log()
    ),
    sum(function (err, value) {
      if (process.env.TEST_VERBOSE) console.log(value)
      t.equal(value, 385)
      t.end()
    })
  )

})

//  pull(
//    values ([1 2 3 4 5 6 7 8 9 10])
//    pull(
//      map({x y;: e*e })
//      log()
//    )
//    sum({
//      err value:
//        t.equal(value 385)
//        t.end()
//      })
//  )
//

tape("writable pull() should throw when called twice", function (t) {
  t.plan(2)

  var stream = pull(
    map(function (e) { return e*e }),
    sum(function (err, value) {
      if (process.env.TEST_VERBOSE) console.log(value)
      t.equal(value, 385)
    })
  )

  stream(values([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]))

  t.throws(function () {
    stream(values([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]))
  }, TypeError)
})
