var stream = require('stream')
var inherits = require('util').inherits
var getLifecycleConfigs = require('./helpers/lifecycle-configs');

inherits(Values, stream.Readable)

function Values (v) {
  this.i = 0
  this.values = v
  stream.Readable.call(this, {objectMode: true})
}

Values.prototype._read = function () {
  if(this.i >= this.values.length)
    this.push(null)
  else
    this.push(this.values[this.i++])
}


inherits(Async, stream.Transform)

function Async (fn) {
  this._map = fn
  stream.Transform.call(this, {objectMode: true})
}

Async.prototype._transform = function (chunk, _, callback) {
  var self = this
  this._map(chunk, function (err, data) {
    self.push(JSON.parse(data))
    //it seems that this HAS to be async, which slows this down a lot.
    setImmediate(callback)
  })
}
Async.prototype._flush = function (callback) {
  this.push(null)
  setImmediate(callback)
}

inherits(Collect, stream.Writable)

function Collect (cb) {
  this._ary = []
  this._cb = cb
  stream.Writable.call(this, {objectMode: true})
}

Collect.prototype._write = function (chunk, _, callback) {
  this._ary.push(chunk)
  setImmediate(callback)
}

//I couldn't figure out which method you are ment to override to implement a writable
//stream so I ended up just using .end and that worked.

//Collect.prototype._destroy = Collect.prototype._final = function (callback) {
//  this._cb(this._ary)
//  callback()
//}
//
//Collect.prototype._flush = function (callback) {
//  this._cb(this._ary)
//  callback()
//}
//
Collect.prototype.end = function () {
  this._cb(null, this._ary)
}

var Benchmark = require('benchmark');
const values = [
  JSON.stringify({ hello: 'world' }),
  JSON.stringify({ foo: 'bar' }),
  JSON.stringify({ bin: 'baz' })
]

module.exports = new Benchmark(
  'node',
  {
    ...getLifecycleConfigs(),
    defer: true,
    fn (deferred) {
    var c = new Collect(function (err, array) {
        if (err) return console.error(err)
        if(array.length < 3) throw new Error('wrong array')
        deferred.resolve()
      })

      new Values(values)
      .pipe(new Async(function (val, done) {
        done(null, val)
      }))
      .pipe(c)
    }
  }
);

if (require.main === module) {
  module.exports.run();
}