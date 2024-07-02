const assert = require('assert')
const { loadConfig } = require('../../lib/config/config-file')

describe('Config file', () => {
  it(`should throw an error if the config file doesn't exist`, () => {
    assert.throws(
      () => loadConfig('.solhint.json'),
      /^Error: The config file passed as a parameter does not exist$/
    )
  })

  it(`should load the config file if exist`, () => {
    const loadedConfig = loadConfig('./test/helpers/solhint_config_test.json')

    const loadedConfigFileExpected = {
      extends: ['solhint:recommended'],
    }

    assert.deepStrictEqual(loadedConfig, loadedConfigFileExpected)
  })
})
