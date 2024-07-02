'use strict'

exports.noSnapshot = function (test, testCommon) {
  function make (run) {
    return function (t) {
      const db = testCommon.factory()
      const operations = [
        { type: 'put', key: 'a', value: 'a' },
        { type: 'put', key: 'b', value: 'b' },
        { type: 'put', key: 'c', value: 'c' }
      ]

      db.open(function (err) {
        t.ifError(err, 'no open error')

        db.batch(operations, function (err) {
          t.ifError(err, 'no batch error')

          // For this test it is important that we don't read eagerly.
          // NOTE: highWaterMarkBytes is not an abstract option, but
          // it is supported by classic-level and others. Also set the
          // old & equivalent leveldown highWaterMark option for compat.
          const it = db.iterator({ highWaterMarkBytes: 0, highWaterMark: 0 })

          run(db, function (err) {
            t.ifError(err, 'no run error')
            verify(t, it, db)
          })
        })
      })
    }
  }

  function verify (t, it, db) {
    it.all(function (err, entries) {
      t.ifError(err, 'no iterator error')

      const kv = entries.map(function ([key, value]) {
        return key.toString() + value.toString()
      })

      if (kv.length === 3) {
        t.same(kv, ['aa', 'bb', 'cc'], 'maybe supports snapshots')
      } else {
        t.same(kv, ['aa', 'cc'], 'ignores keys that have been deleted in the mean time')
      }

      db.close(t.end.bind(t))
    })
  }

  test('delete key after creating iterator', make(function (db, done) {
    db.del('b', done)
  }))

  test('batch delete key after creating iterator', make(function (db, done) {
    db.batch([{ type: 'del', key: 'b' }], done)
  }))
}

exports.all = function (test, testCommon) {
  exports.noSnapshot(test, testCommon)
}
