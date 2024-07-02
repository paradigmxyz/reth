'use strict';

const util = require('util');

const Item = require('./item');
const bufferFrom = require('./buffer').from;
const bufferAlloc = require('./buffer').alloc;

const EMPTY = bufferAlloc(0);
const constants = require('constants');

/**
 * A file.
 * @constructor
 */
function File() {
  Item.call(this);

  /**
   * File content.
   * @type {Buffer}
   */
  this._content = EMPTY;
}
util.inherits(File, Item);

/**
 * Get the file contents.
 * @return {Buffer} File contents.
 */
File.prototype.getContent = function() {
  this.setATime(new Date());
  return this._content;
};

/**
 * Set the file contents.
 * @param {string|Buffer} content File contents.
 */
File.prototype.setContent = function(content) {
  if (typeof content === 'string') {
    content = bufferFrom(content);
  } else if (!Buffer.isBuffer(content)) {
    throw new Error('File content must be a string or buffer');
  }
  this._content = content;
  const now = Date.now();
  this.setCTime(new Date(now));
  this.setMTime(new Date(now));
};

/**
 * Get file stats.
 * @return {Object} Stats properties.
 */
File.prototype.getStats = function() {
  const size = this._content.length;
  const stats = Item.prototype.getStats.call(this);
  stats.mode = this.getMode() | constants.S_IFREG;
  stats.size = size;
  stats.blocks = Math.ceil(size / 512);
  return stats;
};

/**
 * Export the constructor.
 * @type {function()}
 */
exports = module.exports = File;

/**
 * Standard input.
 * @constructor
 */
function StandardInput() {
  File.call(this);
  this.setMode(438); // 0666
}
util.inherits(StandardInput, File);

exports.StandardInput = StandardInput;

/**
 * Standard output.
 * @constructor
 */
function StandardOutput() {
  File.call(this);
  this.setMode(438); // 0666
}
util.inherits(StandardOutput, File);

/**
 * Write the contents to stdout.
 * @param {string|Buffer} content File contents.
 */
StandardOutput.prototype.setContent = function(content) {
  if (process.stdout.isTTY) {
    process.stdout.write(content);
  }
};

exports.StandardOutput = StandardOutput;

/**
 * Standard error.
 * @constructor
 */
function StandardError() {
  File.call(this);
  this.setMode(438); // 0666
}
util.inherits(StandardError, File);

/**
 * Write the contents to stderr.
 * @param {string|Buffer} content File contents.
 */
StandardError.prototype.setContent = function(content) {
  if (process.stderr.isTTY) {
    process.stderr.write(content);
  }
};

exports.StandardError = StandardError;
