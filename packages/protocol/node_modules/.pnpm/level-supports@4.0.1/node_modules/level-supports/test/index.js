'use strict'

const shape = require('./shape')
const cloneable = require('./cloneable')

module.exports = function suite (test, testCommon) {
  test('db has manifest', function (t) {
    const db = testCommon.factory()
    const manifest = db.supports

    shape(t, manifest)
    cloneable(t, manifest)

    const before = Object.assign({}, manifest, {
      additionalMethods: Object.assign({}, manifest.additionalMethods)
    })

    db.open(function (err) {
      t.ifError(err, 'no open error')
      t.same(db.supports, before, 'manifest did not change after open')

      db.close(function (err) {
        t.ifError(err, 'no close error')
        t.same(db.supports, before, 'manifest did not change after close')
        t.end()
      })
    })
  })
}
