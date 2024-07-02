const chalk = require('chalk')
const _ = require('lodash')
const security = require('./security/index')
const naming = require('./naming/index')
const order = require('./order/index')
const bestPractises = require('./best-practises/index')
const deprecations = require('./deprecations/index')
const miscellaneous = require('./miscellaneous/index')
const configObject = require('../config')
const { validSeverityMap } = require('../config/config-validator')

const notifyRuleDeprecated = _.memoize((ruleId, deprecationMessage) => {
  const message = deprecationMessage
    ? `[solhint] Warning: rule '${ruleId}' is deprecated, ${deprecationMessage}.`
    : `[solhint] Warning: rule '${ruleId}' is deprecated.`
  console.warn(chalk.yellow(message))
})

const notifyRuleDoesntExist = _.memoize((ruleId) => {
  console.warn(chalk.yellow(`[solhint] Warning: Rule '${ruleId}' doesn't exist`))
})

module.exports = function checkers(reporter, configVals, inputSrc, tokens, fileName) {
  const config = configObject.from(configVals)
  const { rules } = config
  const meta = {
    reporter,
    config,
    inputSrc,
    tokens,
    fileName,
  }
  const plugins = config.plugins || []

  const allRules = [...coreRules(meta), ...pluginsRules(plugins, meta)]

  const enabledRules = allRules.filter((coreRule) => ruleEnabled(coreRule, rules))

  // show warnings for deprecated rules
  for (const rule of enabledRules) {
    if (rule.meta && rule.meta.deprecated) {
      notifyRuleDeprecated(rule.ruleId, rule.meta.deprecationMessage)
    }
  }

  const configRules = Object.keys(config.rules)
  const allRuleIds = allRules.map((rule) => rule.ruleId)
  for (const rule of configRules) {
    if (!allRuleIds.includes(rule)) {
      notifyRuleDoesntExist(rule)
    }
  }

  return enabledRules
}

function coreRules(meta) {
  const { reporter, config, inputSrc, tokens } = meta

  return [
    ...bestPractises(reporter, config, inputSrc, tokens),
    ...deprecations(reporter),
    ...miscellaneous(reporter, config, tokens),
    ...naming(reporter, config),
    ...order(reporter, tokens),
    ...security(reporter, config, inputSrc),
  ]
}

function loadPlugin(pluginName, { reporter, config, inputSrc, fileName }) {
  let plugins
  try {
    plugins = require(`solhint-plugin-${pluginName}`)
  } catch (e) {
    console.error(
      chalk.red(
        `[solhint] Error: Could not load solhint-plugin-${pluginName}, make sure it's installed.`
      )
    )
    process.exit(1)
  }

  if (!Array.isArray(plugins)) {
    console.warn(
      chalk.yellow(
        `[solhint] Warning: Plugin solhint-plugin-${pluginName} doesn't export an array of rules. Ignoring it.`
      )
    )
    return []
  }

  return plugins
    .map((Plugin) => new Plugin(reporter, config, inputSrc, fileName))
    .map((plugin) => {
      plugin.ruleId = `${pluginName}/${plugin.ruleId}`
      return plugin
    })
}

function pluginsRules(configPlugins, meta) {
  const plugins = Array.isArray(configPlugins) ? configPlugins : [configPlugins]

  return _(plugins)
    .map((name) => loadPlugin(name, meta))
    .flatten()
    .value()
}

function ruleEnabled(coreRule, rules) {
  let ruleValue
  if (rules && !Array.isArray(rules[coreRule.ruleId])) {
    ruleValue = rules[coreRule.ruleId]
  } else if (rules && Array.isArray(rules[coreRule.ruleId])) {
    ruleValue = rules[coreRule.ruleId][0]
  }

  if (
    rules &&
    rules[coreRule.ruleId] !== undefined &&
    ruleValue &&
    validSeverityMap.includes(ruleValue)
  ) {
    return coreRule
  }
}
