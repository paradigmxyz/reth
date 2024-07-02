const fs = require('fs')
const _ = require('lodash')
const { cosmiconfigSync } = require('cosmiconfig')
const { ConfigMissingError } = require('../common/errors')
const packageJson = require('../../package.json')

const getSolhintCoreConfig = (name) => {
  if (name === 'solhint:recommended') {
    return require('../../conf/rulesets/solhint-recommended')
  }

  if (name === 'solhint:all') {
    return require('../../conf/rulesets/solhint-all')
  }

  if (name === 'solhint:default') {
    return require('../../conf/rulesets/solhint-default')
  }

  throw new ConfigMissingError(name)
}

const createEmptyConfig = () => ({
  excludedFiles: {},
  extends: {},
  globals: {},
  env: {},
  rules: {},
  parserOptions: {},
})

const loadConfig = (configFile) => {
  if (configFile && !fs.existsSync(configFile)) {
    throw new Error(`The config file passed as a parameter does not exist`)
  }

  // Use cosmiconfig to get the config from different sources
  const appDirectory = fs.realpathSync(process.cwd())
  const moduleName = packageJson.name
  const cosmiconfigOptions = {
    searchPlaces: [
      'package.json',
      `.${moduleName}.json`,
      `.${moduleName}rc`,
      `.${moduleName}rc.json`,
      `.${moduleName}rc.yaml`,
      `.${moduleName}rc.yml`,
      `.${moduleName}rc.js`,
      `${moduleName}.config.js`,
    ],
  }

  const explorer = cosmiconfigSync(moduleName, cosmiconfigOptions)

  // if a specific path was specified, just load it and ignore default paths
  if (configFile) {
    return explorer.load(configFile).config
  }

  const searchedFor = explorer.search(appDirectory)
  if (!searchedFor) {
    throw new ConfigMissingError()
  }
  return searchedFor.config || createEmptyConfig()
}

const configGetter = (path) =>
  path.startsWith('solhint:') ? getSolhintCoreConfig(path) : require(`solhint-config-${path}`)

const applyExtends = (config, getter = configGetter) => {
  if (!config.extends) {
    return config
  }

  if (!Array.isArray(config.extends)) {
    config.extends = [config.extends]
  }

  return config.extends.reduceRight((previousValue, parentPath) => {
    try {
      const extensionConfig = getter(parentPath)
      return _.merge({}, extensionConfig, previousValue)
    } catch (e) {
      throw new ConfigMissingError(parentPath)
    }
  }, config)
}

module.exports = {
  applyExtends,
  configGetter,
  loadConfig,
}
