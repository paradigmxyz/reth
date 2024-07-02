'use strict'

var cachedown = require('../')
var leveldown = require('memdown')
var tape   = require('tape')
var testCommon = require('abstract-leveldown/testCommon')
var testBuffer = new Buffer('hello')

cachedown.setLeveldown(leveldown)

require('abstract-leveldown/abstract/leveldown-test').args(cachedown, tape)
require('abstract-leveldown/abstract/open-test').args(cachedown, tape, testCommon)
require('abstract-leveldown/abstract/del-test').all(cachedown, tape, testCommon)
require('abstract-leveldown/abstract/put-test').all(cachedown, tape, testCommon)
require('abstract-leveldown/abstract/get-test').all(cachedown, tape, testCommon)
require('abstract-leveldown/abstract/put-get-del-test').all(cachedown, tape, testCommon, testBuffer)
require('abstract-leveldown/abstract/close-test').close(cachedown, tape, testCommon)
require('abstract-leveldown/abstract/iterator-test').all(cachedown, tape, testCommon)

require('abstract-leveldown/abstract/chained-batch-test').all(cachedown, tape, testCommon)
require('abstract-leveldown/abstract/approximate-size-test').setUp(cachedown, tape, testCommon)
require('abstract-leveldown/abstract/approximate-size-test').args(tape)

require('abstract-leveldown/abstract/ranges-test').all(cachedown, tape, testCommon)
require('abstract-leveldown/abstract/batch-test').all(cachedown, tape, testCommon)

require('./custom-tests.js').all(cachedown, tape, testCommon)
