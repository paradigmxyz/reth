const chalk = require('chalk')
const Ajv = require('ajv')

const ruleConfigSchema = (schema) => {
  const baseSchema = {
    type: 'array',
    items: [
      {
        enum: ['error', 'warn', 'off'],
      },
    ],
    minItems: 1,
  }

  if (schema) {
    baseSchema.items.push(schema)
  }

  return baseSchema
}

class BaseChecker {
  constructor(reporter, ruleId, meta) {
    this.reporter = reporter
    this.ruleId = ruleId
    this.meta = meta

    if (!reporter) {
      return
    }

    // validate user's configuration of the rule
    const userConfig = reporter.config[ruleId]
    if (userConfig) {
      if (Array.isArray(userConfig)) {
        const ajv = new Ajv()
        const schema = ruleConfigSchema(meta.schema)
        const validate = ajv.compile(schema)
        if (!validate(userConfig)) {
          const messages = validate.errors
            .map((e) => e.message)
            .filter((x) => x) // filter out possible missing messages
            .map((x) => `  ${x}`) // indent
            .join('\n')

          console.warn(
            chalk.yellow(
              `[solhint] Warning: invalid configuration for rule '${ruleId}':\n${messages}`
            )
          )
        }
      } else if (!['off', 'warn', 'error'].includes(userConfig)) {
        console.warn(
          chalk.yellow(
            `[solhint] Warning: rule '${ruleId}' level is '${userConfig}'; should be one of "error", "warn" or "off"`
          )
        )
      }
    }
  }

  error(ctx, message, fix) {
    this.addReport('error', ctx, message, fix)
  }

  errorAt(line, column, message, fix) {
    this.error({ loc: { start: { line, column } } }, message, fix)
  }

  warn(ctx, message, fix) {
    this.addReport('warn', ctx, message, fix)
  }

  addReport(type, ctx, message, fix) {
    this.reporter[type](ctx, this.ruleId, message, this.meta.fixable ? fix : null)
  }
}

module.exports = BaseChecker
