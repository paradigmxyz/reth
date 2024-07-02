var levelup = require('levelup')
var encode = require('encoding-down')

function packager (leveldown) {
  function Level (location, options, callback) {
    if (typeof options === 'function') {
      callback = options
    }
    if (typeof options !== 'object' || options === null) {
      options = {}
    }

    return levelup(encode(leveldown(location), options), options, callback)
  }

  [ 'destroy', 'repair' ].forEach(function (m) {
    if (typeof leveldown[m] === 'function') {
      Level[m] = function (location, callback) {
        leveldown[m](location, callback || function () {})
      }
    }
  })

  Level.errors = levelup.errors

  return Level
}

module.exports = packager
