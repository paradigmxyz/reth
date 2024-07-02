'use strict'

const { Buffer } = require('buffer')
const { verifyNotFoundError, assertAsync } = require('./util')
const { illegalKeys, illegalValues } = require('./util')

let db

exports.setUp = function (test, testCommon) {
  test('setUp db', function (t) {
    db = testCommon.factory()
    db.open(t.end.bind(t))
  })
}

exports.args = function (test, testCommon) {
  test('test batch() with missing `value`', assertAsync.ctx(function (t) {
    t.plan(3)

    db.batch([{ type: 'put', key: 'foo1' }], assertAsync(function (err) {
      t.is(err && err.code, 'LEVEL_INVALID_VALUE', 'correct error code (callback)')
    }))

    db.batch([{ type: 'put', key: 'foo1' }]).catch((err) => {
      t.is(err.code, 'LEVEL_INVALID_VALUE', 'correct error code (promise)')
    })
  }))

  test('test batch() with illegal values', assertAsync.ctx(function (t) {
    t.plan(illegalValues.length * 6)

    for (const { name, value } of illegalValues) {
      db.batch([{ type: 'put', key: 'foo1', value }], assertAsync(function (err) {
        t.ok(err, name + ' - has error (callback)')
        t.ok(err instanceof Error, name + ' - is Error (callback)')
        t.is(err && err.code, 'LEVEL_INVALID_VALUE', 'correct error code (callback)')
      }))

      db.batch([{ type: 'put', key: 'foo1', value }]).catch(function (err) {
        t.ok(err instanceof Error, name + ' - is Error (promise)')
        t.is(err.code, 'LEVEL_INVALID_VALUE', name + ' - correct error code (promise)')
      })
    }
  }))

  test('test batch() with missing `key`', assertAsync.ctx(function (t) {
    t.plan(3)

    db.batch([{ type: 'put', value: 'foo1' }], assertAsync(function (err) {
      t.is(err && err.code, 'LEVEL_INVALID_KEY', 'correct error code (callback)')
    }))

    db.batch([{ type: 'put', value: 'foo1' }]).catch(function (err) {
      t.is(err.code, 'LEVEL_INVALID_KEY', 'correct error code (promise)')
    })
  }))

  test('test batch() with illegal keys', assertAsync.ctx(function (t) {
    t.plan(illegalKeys.length * 6)

    for (const { name, key } of illegalKeys) {
      db.batch([{ type: 'put', key, value: 'foo1' }], assertAsync(function (err) {
        t.ok(err, name + ' - has error (callback)')
        t.ok(err instanceof Error, name + ' - is Error (callback)')
        t.is(err && err.code, 'LEVEL_INVALID_KEY', 'correct error code (callback)')
      }))

      db.batch([{ type: 'put', key, value: 'foo1' }]).catch(function (err) {
        t.ok(err instanceof Error, name + ' - is Error (promise)')
        t.is(err.code, 'LEVEL_INVALID_KEY', name + ' - correct error code (promise)')
      })
    }
  }))

  test('test batch() with missing or incorrect type', assertAsync.ctx(function (t) {
    t.plan(10)

    db.batch([{ key: 'key', value: 'value' }], assertAsync(function (err) {
      t.is(err && err.name, 'TypeError')
      t.is(err && err.message, "A batch operation must have a type property that is 'put' or 'del'", 'correct error message (callback)')
    }))

    db.batch([{ key: 'key', value: 'value', type: 'foo' }], assertAsync(function (err) {
      t.is(err && err.name, 'TypeError')
      t.is(err && err.message, "A batch operation must have a type property that is 'put' or 'del'", 'correct error message (callback)')
    }))

    db.batch([{ key: 'key', value: 'value' }]).catch(function (err) {
      t.is(err.name, 'TypeError')
      t.is(err.message, "A batch operation must have a type property that is 'put' or 'del'", 'correct error message (promise)')
    })

    db.batch([{ key: 'key', value: 'value', type: 'foo' }]).catch(function (err) {
      t.is(err.name, 'TypeError')
      t.is(err.message, "A batch operation must have a type property that is 'put' or 'del'", 'correct error message (promise)')
    })
  }))

  test('test batch() with missing or nullish operations', assertAsync.ctx(function (t) {
    t.plan(13)

    db.batch(assertAsync(function (err) {
      t.is(err && err.name, 'TypeError')
      t.is(err && err.message, "The first argument 'operations' must be an array", 'correct error message (callback)')
    }))

    for (const array of [null, undefined]) {
      db.batch(array, assertAsync(function (err) {
        t.is(err && err.name, 'TypeError')
        t.is(err && err.message, "The first argument 'operations' must be an array", 'correct error message (callback)')
      }))

      db.batch(array).catch(function (err) {
        t.is(err.name, 'TypeError')
        t.is(err.message, "The first argument 'operations' must be an array", 'correct error message (promise)')
      })
    }
  }))

  test('test batch() with null options', function (t) {
    t.plan(2)

    db.batch([], null, function (err) {
      t.error(err)
    })

    db.batch([], null).then(function () {
      t.pass('resolved')
    }).catch(t.fail.bind(t))
  })

  ;[null, undefined, 1, true].forEach(function (operation) {
    const type = operation === null ? 'null' : typeof operation

    test('test batch() with ' + type + ' operation', assertAsync.ctx(function (t) {
      t.plan(5)

      db.batch([operation], assertAsync(function (err) {
        t.is(err && err.name, 'TypeError')
        t.is(err && err.message, 'A batch operation must be an object', 'correct error message (callback)')
      }))

      db.batch([operation]).catch(function (err) {
        t.is(err.name, 'TypeError')
        t.is(err.message, 'A batch operation must be an object', 'correct error message (promise)')
      })
    }))
  })

  test('test batch() with empty array', assertAsync.ctx(function (t) {
    t.plan(3)

    db.batch([], assertAsync(function (err) {
      t.error(err, 'no error from batch()')
    }))

    db.batch([]).then(function () {
      t.pass('resolved')
    }).catch(t.fail.bind(t))
  }))
}

exports.batch = function (test, testCommon) {
  test('test simple batch()', function (t) {
    db.batch([{ type: 'put', key: 'foo', value: 'bar' }], function (err) {
      t.error(err)

      db.get('foo', function (err, value) {
        t.error(err)
        t.is(value, 'bar')
        t.end()
      })
    })
  })

  test('test simple batch() with promise', async function (t) {
    const db = testCommon.factory()

    await db.open()
    await db.batch([{ type: 'put', key: 'foo', value: 'bar' }])

    t.is(await db.get('foo', { valueEncoding: 'utf8' }), 'bar')
    return db.close()
  })

  test('test multiple batch()', function (t) {
    db.batch([
      { type: 'put', key: 'foobatch1', value: 'bar1' },
      { type: 'put', key: 'foobatch2', value: 'bar2' },
      { type: 'put', key: 'foobatch3', value: 'bar3' },
      { type: 'del', key: 'foobatch2' }
    ], function (err) {
      t.error(err)

      let r = 0
      const done = function () {
        if (++r === 3) { t.end() }
      }

      db.get('foobatch1', function (err, value) {
        t.error(err)
        t.is(value, 'bar1')
        done()
      })

      db.get('foobatch2', function (err, value) {
        t.ok(err, 'entry not found')
        t.ok(typeof value === 'undefined', 'value is undefined')
        t.ok(verifyNotFoundError(err), 'NotFound error')
        done()
      })

      db.get('foobatch3', function (err, value) {
        t.error(err)
        t.is(value, 'bar3')
        done()
      })
    })
  })

  for (const encoding of ['utf8', 'buffer', 'view']) {
    if (!testCommon.supports.encodings[encoding]) continue

    // NOTE: adapted from memdown
    test(`empty values in batch with ${encoding} valueEncoding`, async function (t) {
      const db = testCommon.factory({ valueEncoding: encoding })
      const values = ['', Uint8Array.from([]), Buffer.alloc(0)]
      const expected = encoding === 'utf8' ? values[0] : encoding === 'view' ? values[1] : values[2]

      await db.open()
      await db.batch(values.map((value, i) => ({ type: 'put', key: String(i), value })))

      for (let i = 0; i < values.length; i++) {
        const value = await db.get(String(i))

        // Buffer is a Uint8Array, so this is allowed
        if (encoding === 'view' && Buffer.isBuffer(value)) {
          t.same(value, values[2])
        } else {
          t.same(value, expected)
        }
      }

      return db.close()
    })

    test(`empty keys in batch with ${encoding} keyEncoding`, async function (t) {
      const db = testCommon.factory({ keyEncoding: encoding })
      const keys = ['', Uint8Array.from([]), Buffer.alloc(0)]

      await db.open()

      for (let i = 0; i < keys.length; i++) {
        await db.batch([{ type: 'put', key: keys[i], value: String(i) }])
        t.same(await db.get(keys[i]), String(i), `got value ${i}`)
      }

      return db.close()
    })
  }
}

exports.atomic = function (test, testCommon) {
  test('test batch() is atomic', function (t) {
    t.plan(4)

    let async = false

    db.batch([
      { type: 'put', key: 'foobah1', value: 'bar1' },
      { type: 'put', value: 'bar2' },
      { type: 'put', key: 'foobah3', value: 'bar3' }
    ], function (err) {
      t.ok(err, 'should error')
      t.ok(async, 'callback is asynchronous')

      db.get('foobah1', function (err) {
        t.ok(err, 'should not be found')
      })
      db.get('foobah3', function (err) {
        t.ok(err, 'should not be found')
      })
    })

    async = true
  })
}

exports.events = function (test, testCommon) {
  test('test batch([]) (array-form) emits batch event', async function (t) {
    t.plan(2)

    const db = testCommon.factory()
    await db.open()

    t.ok(db.supports.events.batch)

    db.on('batch', function (ops) {
      t.same(ops, [{ type: 'put', key: 456, value: 99, custom: 123 }])
    })

    await db.batch([{ type: 'put', key: 456, value: 99, custom: 123 }])
    await db.close()
  })
}

exports.tearDown = function (test, testCommon) {
  test('tearDown', function (t) {
    db.close(t.end.bind(t))
  })
}

exports.all = function (test, testCommon) {
  exports.setUp(test, testCommon)
  exports.args(test, testCommon)
  exports.batch(test, testCommon)
  exports.atomic(test, testCommon)
  exports.events(test, testCommon)
  exports.tearDown(test, testCommon)
}
