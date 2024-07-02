const BaseChecker = require('../base-checker')

const ruleId = 'visibility-modifier-order'
const meta = {
  type: 'order',

  docs: {
    description: `Visibility modifier must be first in list of modifiers.`,
    category: 'Style Guide Rules',
    examples: {
      good: [
        {
          description: 'Visibility modifier placed first',
          code: require('../../../test/fixtures/order/visibility-modifier-first'),
        },
      ],
      bad: [
        {
          description: 'Visibility modifier not placed first',
          code: require('../../../test/fixtures/order/visibility-modifier-not-first'),
        },
      ],
    },
  },

  isDefault: false,
  recommended: true,
  defaultSetup: 'warn',

  schema: null,
}

class VisibilityModifierOrderChecker extends BaseChecker {
  constructor(reporter, tokens) {
    super(reporter, ruleId, meta)
    this.tokens = tokens
  }

  getTokensWithoutFunctionParams(node) {
    const parametersCount = node.parameters.length
    const nodeStart = parametersCount
      ? node.parameters[parametersCount - 1].loc.end
      : node.loc.start
    const lastParamIndex = this.tokens.findIndex(
      (token) =>
        token.loc.start.line === nodeStart.line && token.loc.start.column === nodeStart.column
    )

    // discard parameters
    return this.tokens.slice(lastParamIndex + 1)
  }

  FunctionDefinition(node) {
    if (node.visibility !== 'default' && (node.stateMutability || node.modifiers.length)) {
      const functionTokens = []
      const nodeEnd = node.loc.end
      const tokens = this.getTokensWithoutFunctionParams(node)

      for (let i = 0, n = tokens.length; i < n; ++i) {
        const token = tokens[i]

        if (functionTokens.length && token.value === '{') break

        const {
          type,
          loc: { start },
        } = token

        if (start.line <= nodeEnd.line && ['Keyword', 'Identifier'].includes(type)) {
          functionTokens.push(token)
        }
      }

      const visibilityIndex = functionTokens.findIndex((t) => t.value === node.visibility)
      const stateMutabilityIndex = functionTokens.findIndex((t) => t.value === node.stateMutability)
      const modifierIndex = node.modifiers.length
        ? functionTokens.findIndex((t) => t.value === node.modifiers[0].name)
        : -1

      if (
        (stateMutabilityIndex > -1 && visibilityIndex > stateMutabilityIndex) ||
        (modifierIndex > -1 && visibilityIndex > modifierIndex)
      ) {
        this._error(functionTokens[visibilityIndex])
      }
    }
  }

  _error(node) {
    const message = 'Visibility modifier must be first in list of modifiers'
    this.error(node, message)
  }
}

module.exports = VisibilityModifierOrderChecker
