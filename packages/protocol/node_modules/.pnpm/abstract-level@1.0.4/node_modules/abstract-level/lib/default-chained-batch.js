'use strict'

const { AbstractChainedBatch } = require('../abstract-chained-batch')
const ModuleError = require('module-error')
const kEncoded = Symbol('encoded')

// Functional default for chained batch, with support of deferred open
class DefaultChainedBatch extends AbstractChainedBatch {
  constructor (db) {
    super(db)
    this[kEncoded] = []
  }

  _put (key, value, options) {
    this[kEncoded].push({ ...options, type: 'put', key, value })
  }

  _del (key, options) {
    this[kEncoded].push({ ...options, type: 'del', key })
  }

  _clear () {
    this[kEncoded] = []
  }

  // Assumes this[kEncoded] cannot change after write()
  _write (options, callback) {
    if (this.db.status === 'opening') {
      this.db.defer(() => this._write(options, callback))
    } else if (this.db.status === 'open') {
      if (this[kEncoded].length === 0) this.nextTick(callback)
      else this.db._batch(this[kEncoded], options, callback)
    } else {
      this.nextTick(callback, new ModuleError('Batch is not open: cannot call write() after write() or close()', {
        code: 'LEVEL_BATCH_NOT_OPEN'
      }))
    }
  }
}

exports.DefaultChainedBatch = DefaultChainedBatch
