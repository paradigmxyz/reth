'use strict'

const { fromCallback } = require('catering')
const ModuleError = require('module-error')
const { getCallback, getOptions } = require('./lib/common')

const kPromise = Symbol('promise')
const kStatus = Symbol('status')
const kOperations = Symbol('operations')
const kFinishClose = Symbol('finishClose')
const kCloseCallbacks = Symbol('closeCallbacks')

class AbstractChainedBatch {
  constructor (db) {
    if (typeof db !== 'object' || db === null) {
      const hint = db === null ? 'null' : typeof db
      throw new TypeError(`The first argument must be an abstract-level database, received ${hint}`)
    }

    this[kOperations] = []
    this[kCloseCallbacks] = []
    this[kStatus] = 'open'
    this[kFinishClose] = this[kFinishClose].bind(this)

    this.db = db
    this.db.attachResource(this)
    this.nextTick = db.nextTick
  }

  get length () {
    return this[kOperations].length
  }

  put (key, value, options) {
    if (this[kStatus] !== 'open') {
      throw new ModuleError('Batch is not open: cannot call put() after write() or close()', {
        code: 'LEVEL_BATCH_NOT_OPEN'
      })
    }

    const err = this.db._checkKey(key) || this.db._checkValue(value)
    if (err) throw err

    const db = options && options.sublevel != null ? options.sublevel : this.db
    const original = options
    const keyEncoding = db.keyEncoding(options && options.keyEncoding)
    const valueEncoding = db.valueEncoding(options && options.valueEncoding)
    const keyFormat = keyEncoding.format

    // Forward encoding options
    options = { ...options, keyEncoding: keyFormat, valueEncoding: valueEncoding.format }

    // Prevent double prefixing
    if (db !== this.db) {
      options.sublevel = null
    }

    const mappedKey = db.prefixKey(keyEncoding.encode(key), keyFormat)
    const mappedValue = valueEncoding.encode(value)

    this._put(mappedKey, mappedValue, options)
    this[kOperations].push({ ...original, type: 'put', key, value })

    return this
  }

  _put (key, value, options) {}

  del (key, options) {
    if (this[kStatus] !== 'open') {
      throw new ModuleError('Batch is not open: cannot call del() after write() or close()', {
        code: 'LEVEL_BATCH_NOT_OPEN'
      })
    }

    const err = this.db._checkKey(key)
    if (err) throw err

    const db = options && options.sublevel != null ? options.sublevel : this.db
    const original = options
    const keyEncoding = db.keyEncoding(options && options.keyEncoding)
    const keyFormat = keyEncoding.format

    // Forward encoding options
    options = { ...options, keyEncoding: keyFormat }

    // Prevent double prefixing
    if (db !== this.db) {
      options.sublevel = null
    }

    this._del(db.prefixKey(keyEncoding.encode(key), keyFormat), options)
    this[kOperations].push({ ...original, type: 'del', key })

    return this
  }

  _del (key, options) {}

  clear () {
    if (this[kStatus] !== 'open') {
      throw new ModuleError('Batch is not open: cannot call clear() after write() or close()', {
        code: 'LEVEL_BATCH_NOT_OPEN'
      })
    }

    this._clear()
    this[kOperations] = []

    return this
  }

  _clear () {}

  write (options, callback) {
    callback = getCallback(options, callback)
    callback = fromCallback(callback, kPromise)
    options = getOptions(options)

    if (this[kStatus] !== 'open') {
      this.nextTick(callback, new ModuleError('Batch is not open: cannot call write() after write() or close()', {
        code: 'LEVEL_BATCH_NOT_OPEN'
      }))
    } else if (this.length === 0) {
      this.close(callback)
    } else {
      this[kStatus] = 'writing'
      this._write(options, (err) => {
        this[kStatus] = 'closing'
        this[kCloseCallbacks].push(() => callback(err))

        // Emit after setting 'closing' status, because event may trigger a
        // db close which in turn triggers (idempotently) closing this batch.
        if (!err) this.db.emit('batch', this[kOperations])

        this._close(this[kFinishClose])
      })
    }

    return callback[kPromise]
  }

  _write (options, callback) {}

  close (callback) {
    callback = fromCallback(callback, kPromise)

    if (this[kStatus] === 'closing') {
      this[kCloseCallbacks].push(callback)
    } else if (this[kStatus] === 'closed') {
      this.nextTick(callback)
    } else {
      this[kCloseCallbacks].push(callback)

      if (this[kStatus] !== 'writing') {
        this[kStatus] = 'closing'
        this._close(this[kFinishClose])
      }
    }

    return callback[kPromise]
  }

  _close (callback) {
    this.nextTick(callback)
  }

  [kFinishClose] () {
    this[kStatus] = 'closed'
    this.db.detachResource(this)

    const callbacks = this[kCloseCallbacks]
    this[kCloseCallbacks] = []

    for (const cb of callbacks) {
      cb()
    }
  }
}

exports.AbstractChainedBatch = AbstractChainedBatch
