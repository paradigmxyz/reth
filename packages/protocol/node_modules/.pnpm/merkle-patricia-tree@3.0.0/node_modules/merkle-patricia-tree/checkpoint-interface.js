"use strict";

var level = require('level-mem');

var async = require('async');

var WriteStream = require('level-ws');

var callTogether = require('./util').callTogether;

var ScratchReadStream = require('./scratchReadStream');

module.exports = checkpointInterface;

function checkpointInterface(trie) {
  this._scratch = null;
  trie._checkpoints = [];
  Object.defineProperty(trie, 'isCheckpoint', {
    get: function get() {
      return !!trie._checkpoints.length;
    }
  }); // new methods

  trie.checkpoint = checkpoint;
  trie.commit = commit;
  trie.revert = revert;
  trie._enterCpMode = _enterCpMode;
  trie._exitCpMode = _exitCpMode;
  trie.createScratchReadStream = createScratchReadStream; // overwrites

  trie.copy = copy.bind(trie, trie.copy.bind(trie));
}
/**
 * Creates a checkpoint that can later be reverted to or committed. After this is called, no changes to the trie will be permanently saved until `commit` is called
 * @method checkpoint
 * @private
 */


function checkpoint() {
  var self = this;
  var wasCheckpoint = self.isCheckpoint;

  self._checkpoints.push(self.root);

  if (!wasCheckpoint && self.isCheckpoint) {
    self._enterCpMode();
  }
}
/**
 * commits a checkpoint to disk
 * @method commit
 * @private
 * @param {Function} cb the callback
 */


function commit(cb) {
  var self = this;
  cb = callTogether(cb, self.sem.leave);
  self.sem.take(function () {
    if (self.isCheckpoint) {
      self._checkpoints.pop();

      if (!self.isCheckpoint) {
        self._exitCpMode(true, cb);
      } else {
        cb();
      }
    } else {
      throw new Error('trying to commit when not checkpointed');
    }
  });
}
/**
 * Reverts the trie to the state it was at when `checkpoint` was first called.
 * @method revert
 * @private
 * @param {Function} cb the callback
 */


function revert(cb) {
  var self = this;
  cb = callTogether(cb, self.sem.leave);
  self.sem.take(function () {
    if (self.isCheckpoint) {
      self.root = self._checkpoints.pop();

      if (!self.isCheckpoint) {
        self._exitCpMode(false, cb);

        return;
      }
    }

    cb();
  });
} // enter into checkpoint mode


function _enterCpMode() {
  this._scratch = level();
  this._getDBs = [this._scratch].concat(this._getDBs);
  this.__putDBs = this._putDBs;
  this._putDBs = [this._scratch];
  this._putRaw = this.putRaw;
  this.putRaw = putRaw;
} // exit from checkpoint mode


function _exitCpMode(commitState, cb) {
  var self = this;
  var scratch = this._scratch;
  this._scratch = null;
  this._getDBs = this._getDBs.slice(1);
  this._putDBs = this.__putDBs;
  this.putRaw = this._putRaw;

  function flushScratch(db, cb) {
    self.createScratchReadStream(scratch).pipe(WriteStream(db)).on('close', cb);
  }

  if (commitState) {
    async.map(this._putDBs, flushScratch, cb);
  } else {
    cb();
  }
} // adds the interface when copying the trie


function copy(_super) {
  var trie = _super();

  checkpointInterface.call(trie, trie);
  trie._scratch = this._scratch; // trie._checkpoints = this._checkpoints.slice()

  return trie;
}

function putRaw(key, val, cb) {
  function dbPut(db, cb2) {
    db.put(key, val, {
      keyEncoding: 'binary',
      valueEncoding: 'binary'
    }, cb2);
  }

  async.each(this.__putDBs, dbPut, cb);
}

function createScratchReadStream(scratch) {
  var trie = this.copy();
  scratch = scratch || this._scratch; // only read from the scratch

  trie._getDBs = [scratch];
  trie._scratch = scratch;
  return new ScratchReadStream(trie);
}