var pull = require('pull-stream')
var cat = require('../')
var test = require('tape')
var Pushable = require('pull-pushable')
var Abortable = require('pull-abortable')

test('cat', function (t) {
  pull(
    cat([pull.values([1,2,3]), pull.values([4,5,6])]),
    pull.collect(function (err, ary) {
      console.log(err, ary)
      t.notOk(err)
      t.deepEqual(ary, [1,2,3,4,5,6])
      t.end()
    })
  )
})

test('cat - with empty', function (t) {
  pull(
    cat([pull.values([1,2,3]), null, pull.values([4,5,6])]),
    pull.collect(function (err, ary) {
      console.log(err, ary)
      t.notOk(err)
      t.deepEqual(ary, [1,2,3,4,5,6])
      t.end()
    })
  )
})

test('cat - with empty stream', function (t) {
  var ended = false
  var justEnd = function (err, cb) { ended = true; cb(true) }

  pull(
    cat([pull.values([1,2,3]), justEnd, pull.values([4,5,6])]),
    pull.collect(function (err, ary) {
      console.log(err, ary)
      t.ok(ended)
      t.notOk(err)
      t.deepEqual(ary, [1,2,3,4,5,6])
      t.end()
    })
  )
})



test('abort - with empty', function (t) {
  pull(
    cat([pull.values([1,2,3]), null, pull.values([4,5,6])]),
    function (read) {
      read(true, function (err) {
        t.equal(err, true)
        t.end()
      })
    }
  )
})

test('error', function (t) {
  var err = new Error('test error')
  pull(
    cat([pull.values([1,2,3]), function (_, cb) {
      cb(err)
    }]),
    pull.collect(function (_err) {
      console.log('COLLECT END', _err)
      t.equal(_err, err)
      t.end()
    })
  )
})

test('abort stalled', function (t) {
  var err = new Error('intentional'), n = 2
  var abortable = Abortable()
  var pushable = Pushable(function (_err) {
    t.equal(_err, err)
    next()
  })

  pushable.push(4)

  pull(
    cat([pull.values([1,2,3]), undefined, pushable]),
    abortable,
    pull.drain(function (item) {
      if(item == 4)
        process.nextTick(function () {
          abortable.abort(err)
        })
    }, function (err) {
      next()
    })
  )

  function next () {
    if(--n) return
    t.end()
  }
})

test('abort empty', function (t) {
  cat([])(true, function (end) {
    t.equal(end, true)
    t.end()
  })
})

test('error + undefined', function (t) {
  var err = new Error('test error')
  pull(
    cat([pull.values([1,2,3]), function (_, cb) {
      cb(err)
    }, undefined]),
    pull.collect(function (_err) {
      t.equal(_err, err)
      t.end()
    })
  )
})

test('take cat', function (t) {
  pull(
    cat([
      pull(pull.values([1,2,3]), pull.take(2)),
      pull(pull.values([8,7,6,5]), pull.take(3)),
    ]),
    pull.collect(function (err, data) {
      t.error(err)
      t.deepEqual(data, [1,2,8,7,6])
      t.end()
    })
  )
})

test('abort streams after error', function (t) {
  var err = new Error('test error')
  var aborted = false
  pull(
    cat([pull.values([1,2,3]), function (_, cb) {
      cb(err)
    }, function (_err, cb) {
      //this stream should be aborted.
      aborted = true
      t.strictEqual(_err, err)
      cb()
    }]),
    pull.collect(function (_err) {
      t.equal(aborted, true)
      t.equal(_err, err)
      t.end()
    })
  )
})



