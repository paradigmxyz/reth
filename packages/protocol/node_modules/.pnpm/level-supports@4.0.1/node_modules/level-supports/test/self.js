'use strict'

const test = require('tape')
const { supports } = require('..')
const shape = require('./shape')
const cloneable = require('./cloneable')

test('no options', function (t) {
  shape(t, supports())
  cloneable(t, supports())
  t.end()
})

test('falsy options', function (t) {
  ;[null, false, undefined, 0, ''].forEach(function (value) {
    const manifest = supports({
      seek: value,
      additionalMethods: {
        foo: value
      }
    })

    shape(t, manifest)
    t.is(manifest.seek, false)
  })

  t.end()
})

test('truthy options', function (t) {
  ;[true, {}, 'yes', 1, []].forEach(function (value) {
    const manifest = supports({
      streams: value,
      additionalMethods: {
        foo: value
      }
    })

    shape(t, manifest)
    t.same(manifest.streams, value)
    t.same(manifest.additionalMethods.foo, value)
  })

  t.end()
})

test('merges input objects without mutating them', function (t) {
  const input1 = { seek: null, streams: false }
  const input2 = { streams: true, additionalMethods: {} }
  const manifest = supports(input1, input2)

  manifest.foobar = true
  manifest.additionalMethods.baz = true

  t.same(input1, { seek: null, streams: false })
  t.same(input2, { streams: true, additionalMethods: {} })
  t.is(manifest.seek, false)
  t.is(manifest.streams, true)
  shape(t, manifest)
  t.end()
})

test('inherits additionalMethods', function (t) {
  const manifest = supports({ additionalMethods: { foo: true } }, {})
  t.same(manifest.additionalMethods, { foo: true })
  t.end()
})

test('does not merge additionalMethods', function (t) {
  const input1 = { additionalMethods: { foo: true } }
  const input2 = { additionalMethods: { bar: true } }
  const manifest = supports(input1, input2)
  t.same(manifest.additionalMethods, { bar: true })
  t.end()
})
