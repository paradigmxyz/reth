/* global indexedDB */

'use strict'

const { AbstractLevel } = require('abstract-level')
const ModuleError = require('module-error')
const parallel = require('run-parallel-limit')
const { fromCallback } = require('catering')
const { Iterator } = require('./iterator')
const deserialize = require('./util/deserialize')
const clear = require('./util/clear')
const createKeyRange = require('./util/key-range')

// Keep as-is for compatibility with existing level-js databases
const DEFAULT_PREFIX = 'level-js-'

const kIDB = Symbol('idb')
const kNamePrefix = Symbol('namePrefix')
const kLocation = Symbol('location')
const kVersion = Symbol('version')
const kStore = Symbol('store')
const kOnComplete = Symbol('onComplete')
const kPromise = Symbol('promise')

class BrowserLevel extends AbstractLevel {
  constructor (location, options, _) {
    // To help migrating to abstract-level
    if (typeof options === 'function' || typeof _ === 'function') {
      throw new ModuleError('The levelup-style callback argument has been removed', {
        code: 'LEVEL_LEGACY'
      })
    }

    const { prefix, version, ...forward } = options || {}

    super({
      encodings: { view: true },
      snapshots: false,
      createIfMissing: false,
      errorIfExists: false,
      seek: true
    }, forward)

    if (typeof location !== 'string') {
      throw new Error('constructor requires a location string argument')
    }

    // TODO (next major): remove default prefix
    this[kLocation] = location
    this[kNamePrefix] = prefix == null ? DEFAULT_PREFIX : prefix
    this[kVersion] = parseInt(version || 1, 10)
    this[kIDB] = null
  }

  get location () {
    return this[kLocation]
  }

  get namePrefix () {
    return this[kNamePrefix]
  }

  get version () {
    return this[kVersion]
  }

  // Exposed for backwards compat and unit tests
  get db () {
    return this[kIDB]
  }

  get type () {
    return 'browser-level'
  }

  _open (options, callback) {
    const req = indexedDB.open(this[kNamePrefix] + this[kLocation], this[kVersion])

    req.onerror = function () {
      callback(req.error || new Error('unknown error'))
    }

    req.onsuccess = () => {
      this[kIDB] = req.result
      callback()
    }

    req.onupgradeneeded = (ev) => {
      const db = ev.target.result

      if (!db.objectStoreNames.contains(this[kLocation])) {
        db.createObjectStore(this[kLocation])
      }
    }
  }

  [kStore] (mode) {
    const transaction = this[kIDB].transaction([this[kLocation]], mode)
    return transaction.objectStore(this[kLocation])
  }

  [kOnComplete] (request, callback) {
    const transaction = request.transaction

    // Take advantage of the fact that a non-canceled request error aborts
    // the transaction. I.e. no need to listen for "request.onerror".
    transaction.onabort = function () {
      callback(transaction.error || new Error('aborted by user'))
    }

    transaction.oncomplete = function () {
      callback(null, request.result)
    }
  }

  _get (key, options, callback) {
    const store = this[kStore]('readonly')
    let req

    try {
      req = store.get(key)
    } catch (err) {
      return this.nextTick(callback, err)
    }

    this[kOnComplete](req, function (err, value) {
      if (err) return callback(err)

      if (value === undefined) {
        return callback(new ModuleError('Entry not found', {
          code: 'LEVEL_NOT_FOUND'
        }))
      }

      callback(null, deserialize(value))
    })
  }

  _getMany (keys, options, callback) {
    const store = this[kStore]('readonly')
    const tasks = keys.map((key) => (next) => {
      let request

      try {
        request = store.get(key)
      } catch (err) {
        return next(err)
      }

      request.onsuccess = () => {
        const value = request.result
        next(null, value === undefined ? value : deserialize(value))
      }

      request.onerror = (ev) => {
        ev.stopPropagation()
        next(request.error)
      }
    })

    parallel(tasks, 16, callback)
  }

  _del (key, options, callback) {
    const store = this[kStore]('readwrite')
    let req

    try {
      req = store.delete(key)
    } catch (err) {
      return this.nextTick(callback, err)
    }

    this[kOnComplete](req, callback)
  }

  _put (key, value, options, callback) {
    const store = this[kStore]('readwrite')
    let req

    try {
      // Will throw a DataError or DataCloneError if the environment
      // does not support serializing the key or value respectively.
      req = store.put(value, key)
    } catch (err) {
      return this.nextTick(callback, err)
    }

    this[kOnComplete](req, callback)
  }

  // TODO: implement key and value iterators
  _iterator (options) {
    return new Iterator(this, this[kLocation], options)
  }

  _batch (operations, options, callback) {
    const store = this[kStore]('readwrite')
    const transaction = store.transaction
    let index = 0
    let error

    transaction.onabort = function () {
      callback(error || transaction.error || new Error('aborted by user'))
    }

    transaction.oncomplete = function () {
      callback()
    }

    // Wait for a request to complete before making the next, saving CPU.
    function loop () {
      const op = operations[index++]
      const key = op.key

      let req

      try {
        req = op.type === 'del' ? store.delete(key) : store.put(op.value, key)
      } catch (err) {
        error = err
        transaction.abort()
        return
      }

      if (index < operations.length) {
        req.onsuccess = loop
      } else if (typeof transaction.commit === 'function') {
        // Commit now instead of waiting for auto-commit
        transaction.commit()
      }
    }

    loop()
  }

  _clear (options, callback) {
    let keyRange
    let req

    try {
      keyRange = createKeyRange(options)
    } catch (e) {
      // The lower key is greater than the upper key.
      // IndexedDB throws an error, but we'll just do nothing.
      return this.nextTick(callback)
    }

    if (options.limit >= 0) {
      // IDBObjectStore#delete(range) doesn't have such an option.
      // Fall back to cursor-based implementation.
      return clear(this, this[kLocation], keyRange, options, callback)
    }

    try {
      const store = this[kStore]('readwrite')
      req = keyRange ? store.delete(keyRange) : store.clear()
    } catch (err) {
      return this.nextTick(callback, err)
    }

    this[kOnComplete](req, callback)
  }

  _close (callback) {
    this[kIDB].close()
    this.nextTick(callback)
  }
}

BrowserLevel.destroy = function (location, prefix, callback) {
  if (typeof prefix === 'function') {
    callback = prefix
    prefix = DEFAULT_PREFIX
  }

  callback = fromCallback(callback, kPromise)
  const request = indexedDB.deleteDatabase(prefix + location)

  request.onsuccess = function () {
    callback()
  }

  request.onerror = function (err) {
    callback(err)
  }

  return callback[kPromise]
}

exports.BrowserLevel = BrowserLevel
