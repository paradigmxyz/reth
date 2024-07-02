const BaseChecker = require('../base-checker')

const ruleId = 'named-parameters-mapping'
const meta = {
  type: 'naming',
  docs: {
    description: `Solidity v0.8.18 introduced named parameters on the mappings definition.`,
    category: 'Style Guide Rules',
    examples: {
      good: [
        {
          description:
            'To enter "users" mapping the key called "name" is needed to get the "balance"',
          code: 'mapping(string name => uint256 balance) public users;',
        },
        {
          description:
            'To enter owner token balance, the main key "owner" enters another mapping which its key is "token" to get its "balance"',
          code: 'mapping(address owner => mapping(address token => uint256 balance)) public tokenBalances;',
        },
        {
          description:
            'Main key of mapping is enforced. On nested mappings other naming are not neccesary',
          code: 'mapping(address owner => mapping(address => uint256)) public tokenBalances;',
        },
        {
          description:
            'Main key of the parent mapping is enforced. No naming in nested mapping uint256',
          code: 'mapping(address owner => mapping(address token => uint256)) public tokenBalances;',
        },
        {
          description:
            'Main key of the parent mapping is enforced. No naming in nested mapping address',
          code: 'mapping(address owner => mapping(address => uint256 balance)) public tokenBalances;',
        },
      ],
      bad: [
        {
          description: 'No naming at all in regular mapping ',
          code: 'mapping(address => uint256)) public tokenBalances;',
        },
        {
          description: 'Missing any variable name in regular mapping uint256',
          code: 'mapping(address token => uint256)) public tokenBalances;',
        },
        {
          description: 'Missing any variable name in regular mapping address',
          code: 'mapping(address => uint256 balance)) public tokenBalances;',
        },
        {
          description: 'No MAIN KEY naming in nested mapping. Other naming are not enforced',
          code: 'mapping(address => mapping(address token => uint256 balance)) public tokenBalances;',
        },
      ],
    },
  },
  isDefault: false,
  recommended: false,
  defaultSetup: 'off',
  schema: null,
}

class NamedParametersMappingChecker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  StateVariableDeclaration(node) {
    let isNested = false
    const variables = node.variables
    variables.forEach((variable) => {
      // maybe the comparission to VariableDeclaration can be deleted
      if (variable.type === 'VariableDeclaration' && variable.typeName.type === 'Mapping') {
        if (variable.typeName.valueType.type === 'Mapping') {
          // isNested mapping
          isNested = true
        }
        this.checkNameOnMapping(variable, isNested)
      }
    })
  }

  checkNameOnMapping(variable, isNested) {
    let mainKeyName
    let valueName

    if (isNested) {
      mainKeyName = variable.typeName.keyName ? variable.typeName.keyName.name : null
      valueName = variable.typeName.valueType.valueName
        ? variable.typeName.valueType.valueName.name
        : null
    } else {
      mainKeyName = variable.typeName.keyName ? variable.typeName.keyName.name : null
      valueName = variable.typeName.valueName ? variable.typeName.valueName.name : null
    }

    if (!mainKeyName) {
      this.report(variable, 'Main key')
    }

    if (!valueName && !isNested) {
      this.report(variable, 'Value')
    }
  }

  report(variable, type) {
    const message = `${type} parameter in mapping ${variable.name} is not named`
    this.reporter.error(variable, this.ruleId, message)
  }
}

module.exports = NamedParametersMappingChecker
