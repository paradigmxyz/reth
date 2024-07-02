var pull = require('../')
var tape =  require('tape')
tape('async-map', function (t) {

  pull(
    pull.count(),
    pull.take(21),
    pull.asyncMap(function (data, cb) {
      return cb(null, data + 1)
    }),
    pull.collect(function (err, ary) {
      if (process.env.TEST_VERBOSE) console.log(ary)
      t.equal(ary.length, 21)
      t.end()
    })
  )
})

tape('abort async map', function (t) {
  var err = new Error('abort')
  t.plan(2)

  var read = pull(
    pull.infinite(),
    pull.asyncMap(function (data, cb) {
      setImmediate(function () {
        cb(null, data)
      })
    })
  )

  read(null, function (end) {
    if(!end) throw new Error('expected read to end')
    t.ok(end, "read's callback")
  })

  read(err, function (end) {
    if(!end) throw new Error('expected abort to end')
    t.ok(end, "Abort's callback")
    t.end()
  })

})

tape('abort async map (source is slow to ack abort)', function (t) {
  var err = new Error('abort')
  t.plan(3)

  function source(end, cb) {
    if (end) setTimeout(function () { cb(end) }, 20)
    else cb(null, 10)
  }

  var read = pull(
    source,
    pull.asyncMap(function (data, cb) {
      setImmediate(function () {
        cb(null, data)
      })
    })
  )

  var ended = false

  read(null, function (end) {
    if(!end) throw new Error('expected read to end')
    ended = true
    t.ok(end, "read's callback")
  })

  read(err, function (end) {
    if(!end) throw new Error('expected abort to end')
    t.ok(end, "Abort's callback")
    t.ok(ended, 'read called back first')
    t.end()
  })

})

tape('abort async map (async source)', function (t) {
  var err = new Error('abort')
  t.plan(2)

  var read = pull(
    function(err, cb) {
      setImmediate(function() {
        if (err) return cb(err)
        cb(null, 'x')
      })
    },
    pull.asyncMap(function (data, cb) {
      setImmediate(function () {
        cb(null, data)
      })
    })
  )

  read(null, function (end) {
    if(!end) throw new Error('expected read to end')
    t.ok(end, "read's callback")
  })

  read(err, function (end) {
    if(!end) throw new Error('expected abort to end')
    t.ok(end, "Abort's callback")
    t.end()
  })

})
tape('asyncMap aborts when map errors', function (t) {
  t.plan(2)
  var ERR = new Error('abort')
  pull(
    pull.values([1,2,3], function (err) {
      if (process.env.TEST_VERBOSE) console.log('on abort')
      t.equal(err, ERR, 'abort gets error')
      t.end()
    }),
    pull.asyncMap(function (data, cb) {
      cb(ERR)
    }),
    pull.collect(function (err) {
      t.equal(err, ERR, 'collect gets error')
    })
  )
})

tape("async map should pass its own error", function (t) {
  var i = 0
  var error = new Error('error on last call')

  pull(
    function (end, cb) {
      end ? cb(true) : cb(null, i+1)
    },
    pull.asyncMap(function (data, cb) {
      setTimeout(function () {
        if(++i < 5) cb(null, data)
        else {
          cb(error)
        }
      }, 100)
    }),
    pull.collect(function (err, five) {
      t.equal(err, error, 'should return err')
      t.deepEqual(five, [1,2,3,4], 'should skip failed item')
      t.end()
    })
  )
})


