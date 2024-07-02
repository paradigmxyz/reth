const assert = require('assert')
const { applyExtends } = require('../../lib/config/config-file')

describe('applyExtends', () => {
  it('should return the same config if the extends property does not exist', () => {
    const initialConfig = {
      rules: {
        rule0: 'error',
      },
    }
    const result = applyExtends(initialConfig)

    assert.deepStrictEqual(result, initialConfig)
  })

  it('should use the given config if the extends array is empty', () => {
    const initialConfig = {
      extends: [],
      rules: {
        rule0: 'error',
      },
    }
    const result = applyExtends(initialConfig)

    assert.deepStrictEqual(result, initialConfig)
  })

  it('should add the rules in the config', () => {
    const initialConfig = {
      extends: ['config1'],
      rules: {
        rule0: 'error',
      },
    }
    const config1 = {
      rules: {
        rule1: 'warning',
      },
    }
    const result = applyExtends(initialConfig, (configName) => ({ config1 }[configName]))

    assert.deepStrictEqual(result, {
      extends: ['config1'],
      rules: {
        rule0: 'error',
        rule1: 'warning',
      },
    })
  })

  it('should accept a string as the value of extends', () => {
    const initialConfig = {
      extends: 'config1',
      rules: {
        rule0: 'error',
      },
    }
    const config1 = {
      rules: {
        rule1: 'warning',
      },
    }
    const result = applyExtends(initialConfig, (configName) => ({ config1 }[configName]))

    assert.deepStrictEqual(result, {
      extends: ['config1'],
      rules: {
        rule0: 'error',
        rule1: 'warning',
      },
    })
  })

  it('should give higher priority to later configs', () => {
    const initialConfig = {
      extends: ['config1', 'config2'],
      rules: {
        rule0: 'error',
      },
    }
    const config1 = {
      rules: {
        rule1: 'warning',
      },
    }
    const config2 = {
      rules: {
        rule1: 'off',
      },
    }
    const result = applyExtends(initialConfig, (configName) => ({ config1, config2 }[configName]))

    assert.deepStrictEqual(result, {
      extends: ['config1', 'config2'],
      rules: {
        rule0: 'error',
        rule1: 'off',
      },
    })
  })

  it('should give higher priority to rules in the config file', () => {
    const initialConfig = {
      extends: ['config1', 'config2'],
      rules: {
        rule0: 'error',
      },
    }
    const config1 = {
      rules: {
        rule0: 'warning',
      },
    }
    const config2 = {
      rules: {
        rule0: 'off',
      },
    }
    const result = applyExtends(initialConfig, (configName) => ({ config1, config2 }[configName]))

    assert.deepStrictEqual(result, {
      extends: ['config1', 'config2'],
      rules: {
        rule0: 'error',
      },
    })
  })
})
