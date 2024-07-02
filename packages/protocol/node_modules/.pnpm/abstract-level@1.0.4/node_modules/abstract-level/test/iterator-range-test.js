'use strict'

let db

const data = (function () {
  const d = []
  let i = 0
  let k
  for (; i < 100; i++) {
    k = (i < 10 ? '0' : '') + i
    d.push({
      key: k,
      value: String(Math.random())
    })
  }
  return d
}())

exports.setUp = function (test, testCommon) {
  test('setUp db', function (t) {
    db = testCommon.factory()
    db.open(function () {
      db.batch(data.map(function (d) {
        return {
          type: 'put',
          key: d.key,
          value: d.value
        }
      }), t.end.bind(t))
    })
  })
}

exports.range = function (test, testCommon) {
  function rangeTest (name, opts, expected) {
    opts.keyEncoding = 'utf8'
    opts.valueEncoding = 'utf8'

    test(name, function (t) {
      db.iterator(opts).all(function (err, entries) {
        t.error(err)
        t.is(entries.length, expected.length, 'correct number of entries')
        t.same(entries, expected.map(o => [o.key, o.value]))
        t.end()
      })
    })

    // Test the documented promise that in reverse mode,
    // "the returned entries are the same, but in reverse".
    if (!opts.reverse && !('limit' in opts)) {
      const reverseOpts = Object.assign({}, opts, { reverse: true })

      rangeTest(
        name + ' (flipped)',
        reverseOpts,
        expected.slice().reverse()
      )
    }
  }

  rangeTest('test full data collection', {}, data)

  rangeTest('test iterator with reverse=true', {
    reverse: true
  }, data.slice().reverse())

  rangeTest('test iterator with gte=00', {
    gte: '00'
  }, data)

  rangeTest('test iterator with gte=50', {
    gte: '50'
  }, data.slice(50))

  rangeTest('test iterator with lte=50 and reverse=true', {
    lte: '50',
    reverse: true
  }, data.slice().reverse().slice(49))

  rangeTest('test iterator with gte=49.5 (midway)', {
    gte: '49.5'
  }, data.slice(50))

  rangeTest('test iterator with gte=49999 (midway)', {
    gte: '49999'
  }, data.slice(50))

  rangeTest('test iterator with lte=49.5 (midway) and reverse=true', {
    lte: '49.5',
    reverse: true
  }, data.slice().reverse().slice(50))

  rangeTest('test iterator with lt=49.5 (midway) and reverse=true', {
    lt: '49.5',
    reverse: true
  }, data.slice().reverse().slice(50))

  rangeTest('test iterator with lt=50 and reverse=true', {
    lt: '50',
    reverse: true
  }, data.slice().reverse().slice(50))

  rangeTest('test iterator with lte=50', {
    lte: '50'
  }, data.slice(0, 51))

  rangeTest('test iterator with lte=50.5 (midway)', {
    lte: '50.5'
  }, data.slice(0, 51))

  rangeTest('test iterator with lte=50555 (midway)', {
    lte: '50555'
  }, data.slice(0, 51))

  rangeTest('test iterator with lt=50555 (midway)', {
    lt: '50555'
  }, data.slice(0, 51))

  rangeTest('test iterator with gte=50.5 (midway) and reverse=true', {
    gte: '50.5',
    reverse: true
  }, data.slice().reverse().slice(0, 49))

  rangeTest('test iterator with gt=50.5 (midway) and reverse=true', {
    gt: '50.5',
    reverse: true
  }, data.slice().reverse().slice(0, 49))

  rangeTest('test iterator with gt=50 and reverse=true', {
    gt: '50',
    reverse: true
  }, data.slice().reverse().slice(0, 49))

  // first key is actually '00' so it should avoid it
  rangeTest('test iterator with lte=0', {
    lte: '0'
  }, [])

  // first key is actually '00' so it should avoid it
  rangeTest('test iterator with lt=0', {
    lt: '0'
  }, [])

  rangeTest('test iterator with gte=30 and lte=70', {
    gte: '30',
    lte: '70'
  }, data.slice(30, 71))

  // The gte and lte options should take precedence over gt and lt respectively.
  rangeTest('test iterator with gte=30 and lte=70 and gt=40 and lt=60', {
    gte: '30',
    lte: '70',
    gt: '40',
    lt: '60'
  }, data.slice(30, 71))

  // Also test the other way around: if gt and lt were to select a bigger range.
  rangeTest('test iterator with gte=30 and lte=70 and gt=20 and lt=80', {
    gte: '30',
    lte: '70',
    gt: '20',
    lt: '80'
  }, data.slice(30, 71))

  rangeTest('test iterator with gt=29 and lt=71', {
    gt: '29',
    lt: '71'
  }, data.slice(30, 71))

  rangeTest('test iterator with gte=30 and lte=70 and reverse=true', {
    lte: '70',
    gte: '30',
    reverse: true
  }, data.slice().reverse().slice(29, 70))

  rangeTest('test iterator with gt=29 and lt=71 and reverse=true', {
    lt: '71',
    gt: '29',
    reverse: true
  }, data.slice().reverse().slice(29, 70))

  rangeTest('test iterator with limit=20', {
    limit: 20
  }, data.slice(0, 20))

  rangeTest('test iterator with limit=20 and gte=20', {
    limit: 20,
    gte: '20'
  }, data.slice(20, 40))

  rangeTest('test iterator with limit=20 and reverse=true', {
    limit: 20,
    reverse: true
  }, data.slice().reverse().slice(0, 20))

  rangeTest('test iterator with limit=20 and lte=79 and reverse=true', {
    limit: 20,
    lte: '79',
    reverse: true
  }, data.slice().reverse().slice(20, 40))

  // the default limit value from levelup is -1
  rangeTest('test iterator with limit=-1 should iterate over whole database', {
    limit: -1
  }, data)

  rangeTest('test iterator with limit=0 should not iterate over anything', {
    limit: 0
  }, [])

  rangeTest('test iterator with lte after limit', {
    limit: 20,
    lte: '50'
  }, data.slice(0, 20))

  rangeTest('test iterator with lte before limit', {
    limit: 50,
    lte: '19'
  }, data.slice(0, 20))

  rangeTest('test iterator with gte after database end', {
    gte: '9a'
  }, [])

  rangeTest('test iterator with gt after database end', {
    gt: '9a'
  }, [])

  rangeTest('test iterator with lte after database end and reverse=true', {
    lte: '9a',
    reverse: true
  }, data.slice().reverse())

  rangeTest('test iterator with lt after database end', {
    lt: 'a'
  }, data.slice())

  rangeTest('test iterator with lt at database end', {
    lt: data[data.length - 1].key
  }, data.slice(0, -1))

  rangeTest('test iterator with lte at database end', {
    lte: data[data.length - 1].key
  }, data.slice())

  rangeTest('test iterator with lt before database end', {
    lt: data[data.length - 2].key
  }, data.slice(0, -2))

  rangeTest('test iterator with lte before database end', {
    lte: data[data.length - 2].key
  }, data.slice(0, -1))

  rangeTest('test iterator with lte and gte after database and reverse=true', {
    lte: '9b',
    gte: '9a',
    reverse: true
  }, [])

  rangeTest('test iterator with lt and gt after database and reverse=true', {
    lt: '9b',
    gt: '9a',
    reverse: true
  }, [])

  rangeTest('gt greater than lt', {
    gt: '20',
    lt: '10'
  }, [])

  rangeTest('gte greater than lte', {
    gte: '20',
    lte: '10'
  }, [])
}

exports.tearDown = function (test, testCommon) {
  test('tearDown', function (t) {
    db.close(t.end.bind(t))
  })
}

exports.all = function (test, testCommon) {
  exports.setUp(test, testCommon)
  exports.range(test, testCommon)
  exports.tearDown(test, testCommon)
}
