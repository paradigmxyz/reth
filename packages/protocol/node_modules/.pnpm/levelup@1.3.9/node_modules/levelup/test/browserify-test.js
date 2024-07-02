/* Copyright (c) 2012-2016 LevelUP contributors
 * See list at <https://github.com/level/levelup#contributing>
 * MIT License <https://github.com/level/levelup/blob/master/LICENSE.md>
 */

var assert = require('referee').assert
var refute = require('referee').refute
var buster = require('bustermove')
var browserify = require('browserify')
var path = require('path')
var after = require('after')
var bl = require('bl')
var spawn = require('child_process').spawn
var PACKAGE_JSON = path.join(__dirname, '..', 'package.json')

buster.testCase('Browserify Bundle', {
  'does not contain package.json': function (done) {
    var b = browserify(path.join(__dirname, '..'), { browserField: true })
      .once('error', function (error) {
        assert.fail(error)
        done()
      })
    b.pipeline
      .on('file', function (file, id, parent) {
        refute.equals(file, PACKAGE_JSON)
      })
    b.bundle(done)
  },
  'throws error if missing db factory': function (done) {
    var b = browserify(path.join(__dirname, 'data/browser-throws.js'), { browserField: true })
    var node = spawn('node')
    var fin = after(2, done)
    node.stderr.pipe(bl(function (err, buf) {
      refute(err)
      assert.match(buf.toString(), /LevelUPError: missing db factory, you need to set options\.db/)
      fin()
    }))
    node.on('exit', function (code) {
      assert.equals(code, 1)
      fin()
    })
    b.bundle().pipe(node.stdin)
  },
  'works with valid db factory (memdown)': function (done) {
    var b = browserify(path.join(__dirname, 'data/browser-works.js'), { browserField: true })
    var node = spawn('node')
    node.on('exit', function (code) {
      assert.equals(code, 0)
      done()
    })
    b.bundle().pipe(node.stdin)
  }
})
