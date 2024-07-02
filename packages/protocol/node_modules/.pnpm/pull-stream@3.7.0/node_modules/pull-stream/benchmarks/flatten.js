const faker = require('faker');
const pull = require('../');
const Benchmark = require('benchmark');
const getLifecycleConfigs = require('./helpers/lifecycle-configs');

faker.seed(24849320);
function getRandomValues(nestLevel) {
  return Array(faker.datatype.number(1000) + 1)
  .fill(1)
  .map(e => faker.datatype.number());
}

let values;
function setValues () {
  values = pull.values(
    new Array(500)
    .fill(1)
    .map(e => pull.values(getRandomValues()))
  );
}

module.exports = new Benchmark('flatten',
function() {
  pull(
    values,
    pull.flatten(),
    pull.drain()
  );
}, getLifecycleConfigs(setValues));

if (require.main === module) {
  module.exports.run();
}
