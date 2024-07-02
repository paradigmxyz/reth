var pull     = require('pull-stream/pull')
var Map      = require('pull-stream/throughs/map')
var AsyncMap = require('pull-stream/throughs/async-map')
var Drain    = require('pull-stream/sinks/drain')
var Window   = require('pull-window')

module.exports = function (db, opts, done) {
  if('function' === typeof opts)
    done = opts, opts = null
  opts = opts || {}
  return pull(
    Map(function (e) {
      if(e.type) return e
      return {
        key   : e.key, 
        value : e.value,
        type  : e.value == null ? 'del' : 'put'
      }
    }),
    Window.recent(opts.windowSize, opts.windowTime),
    AsyncMap(function (batch, cb) {
      db.batch(batch, cb)
    }),
    Drain(null, done)
  )
}

