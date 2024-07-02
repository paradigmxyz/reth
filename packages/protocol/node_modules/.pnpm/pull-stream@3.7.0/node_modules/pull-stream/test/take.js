var pull = require('../')
var test = require('tape')

test('through - onEnd', function (t) {
  t.plan(2)
  var values = [1,2,3,4,5,6,7,8,9,10]

  //read values, and then just stop!
  //this is a subtle edge case for take!

//I did have a thing that used this edge case,
//but it broke take, actually. so removing it.
//TODO: fix that thing - was a test for some level-db stream thing....

//  pull.Source(function () {
//    return function (end, cb) {
//      if(end) cb(end)
//      else if(values.length)
//        cb(null, values.shift())
//      else console.log('drop')
//    }
//  })()

  pull(
    pull.values(values),
    pull.take(10),
    pull.through(null, function (err) {
      if (process.env.TEST_VERBOSE) console.log('end')
      t.ok(true)
      process.nextTick(function () {
        t.end()
      })
    }),
    pull.collect(function (err, ary) {
      if (process.env.TEST_VERBOSE) console.log(ary)
      t.ok(true)
    })
  )
})


test('take - exclude last (default)', function (t) {
  pull(
    pull.values([1,2,3,4,5,6,7,8,9,10]),
    pull.take(function(n) {return n<5}),
    pull.collect(function (err, four) {
      t.deepEqual(four, [1,2,3,4])
      t.end()
    })
  )
})
test('take - include last', function (t) {
  pull(
    pull.values([1,2,3,4,5,6,7,8,9,10]),
    pull.take(function(n) {return n<5}, {last: true}),
    pull.collect(function (err, five) {
      t.deepEqual(five, [1,2,3,4,5])
      t.end()
    })
  )
})

test('take 5 causes 5 reads upstream', function (t) {
  var reads = 0
  pull(
    pull.values([1,2,3,4,5,6,7,8,9,10]),
    function (read) {
      return function (end, cb) {
        if (end !== true) reads++
        if (process.env.TEST_VERBOSE) console.log(reads, end)
        read(end, cb)
      }
    },
    pull.take(5),
    pull.collect(function (err, five) {
      t.deepEqual(five, [1,2,3,4,5])
      process.nextTick(function() {
          t.equal(reads, 5)
          t.end()
        })
    })
  )
})

test("take doesn't abort until the last read", function (t) {

  var aborted = false

  var ary = [1,2,3,4,5], i = 0

  var read = pull(
    function (abort, cb) {
      if(abort) cb(aborted = true)
      else if(i > ary.length) cb(true)
      else cb(null, ary[i++])
    },
    pull.take(function (d) {
      return d < 3
    }, {last: true})
  )

  read(null, function (_, d) {
    t.notOk(aborted, "hasn't aborted yet")
    read(null, function (_, d) {
      t.notOk(aborted, "hasn't aborted yet")
      read(null, function (_, d) {
        t.notOk(aborted, "hasn't aborted yet")
        read(null, function (end, d) {
          t.ok(end, 'stream ended')
          t.equal(d, undefined, 'data undefined')
          t.ok(aborted, "has aborted by now")
          t.end()
        })
      })
    })
  })

})

test('take should throw error on last read', function (t) {
  var i = 0
  var error = new Error('error on last call')

  pull(
    pull.values([1,2,3,4,5,6,7,8,9,10]),
    pull.take(function(n) {return n<5}, {last: true}),
    // pull.take(5),
    pull.asyncMap(function (data, cb) {
      setTimeout(function () {
        if(++i < 5) cb(null, data)
        else cb(error)
      }, 100)
    }),
    pull.collect(function (err, five) {
      t.equal(err, error, 'should return err')
      t.deepEqual(five, [1,2,3,4], 'should skip failed item')
      t.end()
    })
  )
})
