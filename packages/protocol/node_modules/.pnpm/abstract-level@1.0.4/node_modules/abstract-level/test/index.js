'use strict'

const common = require('./common')
const kSublevels = Symbol('sublevels')

function suite (options) {
  const testCommon = common(options)
  const test = testCommon.test

  require('./factory-test')(test, testCommon)
  require('./manifest-test')(test, testCommon)
  require('./open-test').all(test, testCommon)
  require('./close-test').all(test, testCommon)

  if (testCommon.supports.createIfMissing) {
    require('./open-create-if-missing-test').all(test, testCommon)
  }

  if (testCommon.supports.errorIfExists) {
    require('./open-error-if-exists-test').all(test, testCommon)
  }

  require('./put-test').all(test, testCommon)
  require('./get-test').all(test, testCommon)
  require('./del-test').all(test, testCommon)
  require('./put-get-del-test').all(test, testCommon)
  require('./get-many-test').all(test, testCommon)

  require('./batch-test').all(test, testCommon)
  require('./chained-batch-test').all(test, testCommon)

  require('./iterator-test').all(test, testCommon)
  require('./iterator-range-test').all(test, testCommon)
  require('./async-iterator-test').all(test, testCommon)

  require('./deferred-open-test').all(test, testCommon)
  require('./encoding-test').all(test, testCommon)
  require('./encoding-json-test').all(test, testCommon)
  require('./encoding-custom-test').all(test, testCommon)
  require('./encoding-buffer-test').all(test, testCommon)
  require('./encoding-decode-error-test').all(test, testCommon)

  if (testCommon.supports.seek) {
    require('./iterator-seek-test').all(test, testCommon)
  }

  if (testCommon.supports.snapshots) {
    require('./iterator-snapshot-test').all(test, testCommon)
  } else {
    require('./iterator-no-snapshot-test').all(test, testCommon)
  }

  require('./clear-test').all(test, testCommon)
  require('./clear-range-test').all(test, testCommon)
  require('./sublevel-test').all(test, testCommon)

  // Run the same suite on a sublevel
  if (!testCommon.internals[kSublevels]) {
    const factory = testCommon.factory

    suite({
      ...testCommon,
      internals: { [kSublevels]: true },
      factory (opts) {
        return factory().sublevel('test', opts)
      }
    })
  }
}

suite.common = common
module.exports = suite
