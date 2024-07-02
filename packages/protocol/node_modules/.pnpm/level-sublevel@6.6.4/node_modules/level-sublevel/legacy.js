var nut   = require('./nut')
var shell = require('./shell') //the shell surrounds the nut
var Codec = require('level-codec')
var merge = require('xtend')

var ReadStream = require('./read-stream')

var precodec = require('./codec/legacy')

module.exports = function (db, opts) {

  opts = merge(db.options, opts)

  return shell ( nut ( db, precodec, new Codec ), [], ReadStream, db.options)

}

