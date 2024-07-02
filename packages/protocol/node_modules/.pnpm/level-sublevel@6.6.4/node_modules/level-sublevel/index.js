"use strict";


var nut   = require('./nut')
var shell = require('./shell') //the shell surrounds the nut
var precodec = require('./codec')
var Codec = require('level-codec')
var merge = require('xtend')

var ReadStream = require('./read-stream')

var sublevel = function (db, opts) {
  opts = merge(db.options, opts)
  return shell ( nut ( db, precodec, new Codec ), [], ReadStream, opts)
}

module.exports = function (db, opts) {
  if (typeof db.sublevel === 'function' && typeof db.clone === 'function') return db.clone(opts)
  return sublevel(db, opts)
}
