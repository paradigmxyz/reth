'use strict'

var sources  = require('./sources')
var sinks    = require('./sinks')
var throughs = require('./throughs')

exports = module.exports = require('./pull')

exports.pull = exports

for(var k in sources)
  exports[k] = sources[k]

for(var k in throughs)
  exports[k] = throughs[k]

for(var k in sinks)
  exports[k] = sinks[k]

