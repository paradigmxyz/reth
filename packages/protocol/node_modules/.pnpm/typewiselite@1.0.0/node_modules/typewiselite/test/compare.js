'use strict';

var typewise = require('../');
var util = require('./util');
var tape = require('tape');

tape('flat', function (t) {
  var sample = util.getSample();
  var shuffled = util.shuffle(sample.slice());
  t.deepEqual(shuffled.sort(typewise), sample)
  t.end()
})


tape('arrays', function (t) {
  var sample = util.getArraySample(1);
  var shuffled = util.shuffle(sample.slice());
  t.deepEqual(shuffled.sort(typewise), sample);
  t.end()
})

tape('nested arrays', function (t) {
  var sample = util.getArraySample(2);
  var shuffled = util.shuffle(sample.slice());
  t.deepEqual(shuffled.sort(typewise), sample);
  t.end()
})


