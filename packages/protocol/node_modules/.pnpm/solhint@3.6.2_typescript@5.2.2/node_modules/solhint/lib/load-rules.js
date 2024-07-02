const path = require('path')
const { walkSync } = require('./common/utils')

const blocklistedFiles = ['index.js', 'base-deprecation.js', 'load-rules.js']

/**
 * Load all rule modules from specified directory
 */
const loadRules = () => {
  const rulesDir = path.join(__dirname, 'rules')
  const rules = []
  const files = walkSync(rulesDir)

  const filesFiltered = files.filter((file) => {
    const filename = path.parse(file).base
    return path.extname(filename) === '.js' && !blocklistedFiles.includes(filename)
  })

  for (const file of filesFiltered) {
    const FileRule = require(file)

    const isClass = typeof FileRule === 'function'
    if (!isClass) {
      return
    }

    const instance = new FileRule()
    if (instance && instance.ruleId) {
      rules.push({ ruleId: instance.ruleId, meta: instance.meta, file })
    }
  }

  return rules
}

const loadRule = (rule) => {
  const rulesDir = path.join(__dirname, 'rules')
  let fileInstance
  const files = walkSync(rulesDir)

  const filesFiltered = files.filter((file) => {
    const filename = path.parse(file).base
    return path.extname(filename) === '.js' && !blocklistedFiles.includes(filename)
  })

  for (const file of filesFiltered) {
    const filename = path.parse(file).name
    if (filename !== rule) {
      continue
    }
    const FileRule = require(file)

    const isClass = typeof FileRule === 'function'
    if (!isClass) {
      continue
    }

    fileInstance = new FileRule()
  }

  return fileInstance
}

module.exports = {
  loadRules,
  loadRule,
}
