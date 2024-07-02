/* eslint-disable no-fallthrough */
/* eslint-disable default-case */
const BaseChecker = require('../base-checker')
const { hasMethodCalls } = require('../../common/tree-traversing')
const LOW_LEVEL_CALLS = require('../../../test/fixtures/security/low-level-calls')

const WARN_LOW_LEVEL_CALLS = LOW_LEVEL_CALLS[0]
const ALLOWED_LOW_LEVEL_CALLS = LOW_LEVEL_CALLS[1]

const ruleId = 'avoid-low-level-calls'
const meta = {
  type: 'security',

  docs: {
    description: `Avoid to use low level calls.`,
    category: 'Security Rules',
    examples: {
      good: [
        {
          description: 'Using low level calls to transfer funds',
          code: ALLOWED_LOW_LEVEL_CALLS.join('\n'),
        },
      ],
      bad: [
        {
          description: 'Using low level calls',
          code: WARN_LOW_LEVEL_CALLS.join('\n'),
        },
      ],
    },
  },

  isDefault: false,
  recommended: true,
  defaultSetup: 'warn',

  schema: null,
}

class AvoidLowLevelCallsChecker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  FunctionCall(node) {
    switch (node.expression.type) {
      case 'MemberAccess':
        if (
          // can add 'value' in the array of hasMethodCalls() and remove the || condition but seems out of place
          hasMethodCalls(node.expression, ['call', 'callcode', 'delegatecall']) ||
          (node.expression.expression.memberName === 'call' && // this is for anyAddress.call.value(code)()
            node.expression.memberName === 'value')
        ) {
          return this._warn(node)
        }
      case 'NameValueExpression':
        // Allow low level method calls for transferring funds
        //
        // See:
        //
        // - https://solidity-by-example.org/sending-ether/
        // - https://consensys.net/diligence/blog/2019/09/stop-using-soliditys-transfer-now/
        if (
          node.expression.expression.memberName === 'call' &&
          node.expression.arguments.type === 'NameValueList' &&
          node.expression.arguments.names.includes('value')
        ) {
          return
        }

        if (hasMethodCalls(node.expression.expression, ['call', 'callcode', 'delegatecall'])) {
          return this._warn(node)
        }
    }
  }

  _warn(ctx) {
    const message = 'Avoid to use low level calls.'
    this.warn(ctx, message)
  }
}

module.exports = AvoidLowLevelCallsChecker
