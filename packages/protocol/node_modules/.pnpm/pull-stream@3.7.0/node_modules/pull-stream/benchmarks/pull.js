const Benchmark = require('benchmark')
const pull = require('../')
var getLifecycleConfigs = require('./helpers/lifecycle-configs');

const values = [
  JSON.stringify({ hello: 'world' }),
  JSON.stringify({ foo: 'bar' }),
  JSON.stringify({ bin: 'baz' })
]

module.exports = new Benchmark('pull', {
  ...getLifecycleConfigs(),
  defer: true,
  fn (deferred) {
    const source = pull.values(values)
    const through = pull.asyncMap(function (val, done) {
      const json = JSON.parse(val)
      done(null, json)
    })

    const sink = pull.collect(function (err, array) {
      if (err) return console.error(err)
      deferred.resolve()
    })
    pull(source, through, sink)
  }
});

// const run = bench([
  /*,
  function pull_compose (done) {
    const source = pull.values(values)
    const through = pull.asyncMap(function (val, done) {
      const json = JSON.parse(val)
      done(null, json)
    })

    const sink = pull.collect(function (err, array) {
      if (err) return console.error(err)
      setImmediate(done)
    })
    pull(source, pull(through, sink))
  },
  function pull_chain (done) {
    const source = pull.values(values)
    const through = pull.asyncMap(function (val, done) {
      const json = JSON.parse(val)
      done(null, json)
    })

    const sink = pull.collect(function (err, array) {
      if (err) return console.error(err)
      setImmediate(done)
    })
    pull(pull(source, through), sink)
  }*/
// ], N=100000)

if (require.main === module) {
  module.exports.run();
}