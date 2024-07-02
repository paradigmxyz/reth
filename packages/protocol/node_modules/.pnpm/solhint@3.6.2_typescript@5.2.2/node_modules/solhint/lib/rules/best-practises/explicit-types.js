const BaseChecker = require('../base-checker')
const { severityDescription, formatEnum } = require('../../doc/utils')

const EXPLICIT_TYPES = ['uint256', 'int256', 'ufixed128x18', 'fixed128x18']
const IMPLICIT_TYPES = ['uint', 'int', 'ufixed', 'fixed']
const ALL_TYPES = EXPLICIT_TYPES.concat(IMPLICIT_TYPES)
const VALID_CONFIGURATION_OPTIONS = ['explicit', 'implicit']
const DEFAULT_OPTION = 'explicit'
const DEFAULT_SEVERITY = 'warn'

const ruleId = 'explicit-types'
const meta = {
  type: 'best-practises',

  docs: {
    description: 'Forbid or enforce explicit types (like uint256) that have an alias (like uint).',
    category: 'Best Practise Rules',
    options: [
      {
        description: severityDescription,
        default: DEFAULT_SEVERITY,
      },
      {
        description: `Options need to be one of ${formatEnum(VALID_CONFIGURATION_OPTIONS)}`,
        default: DEFAULT_OPTION,
      },
    ],
    examples: {
      good: [
        {
          description: 'If explicit is selected',
          code: 'uint256 public variableName',
        },
        {
          description: 'If implicit is selected',
          code: 'uint public variableName',
        },
      ],
      bad: [
        {
          description: 'If explicit is selected',
          code: 'uint public variableName',
        },
        {
          description: 'If implicit is selected',
          code: 'uint256 public variableName',
        },
      ],
    },
  },

  isDefault: false,
  recommended: true,
  defaultSetup: [DEFAULT_SEVERITY, DEFAULT_OPTION],

  schema: {
    type: 'string',
    enum: VALID_CONFIGURATION_OPTIONS,
  },
}

class ExplicitTypesChecker extends BaseChecker {
  constructor(reporter, config) {
    super(reporter, ruleId, meta)
    this.configOption =
      (config && config.getStringFromArray(ruleId, DEFAULT_OPTION)) || DEFAULT_OPTION
    this.isExplicit = this.configOption === 'explicit'
  }

  VariableDeclaration(node) {
    if (!VALID_CONFIGURATION_OPTIONS.includes(this.configOption)) {
      this.error(node, 'Error: Config error on [explicit-types]. See explicit-types.md.')
      return
    }

    const typeToFind = 'ElementaryTypeName'
    const onlyTypes = this.findNamesOfElementaryTypeName(node, typeToFind)

    // if no variables are defined, return
    if (onlyTypes && onlyTypes.length === 0) return

    // this tells if the variable needs to be checked
    const varsToBeChecked = onlyTypes.filter((type) => ALL_TYPES.includes(type))

    // if defined variables are not to be checked (example address type), return
    if (varsToBeChecked && varsToBeChecked.length === 0) return

    // if explicit, it will search for implicit and viceversa
    if (this.isExplicit) {
      this.validateVariables(IMPLICIT_TYPES, node, varsToBeChecked)
    } else {
      this.validateVariables(EXPLICIT_TYPES, node, varsToBeChecked)
    }
  }

  validateVariables(configType, node, varsToBeChecked) {
    const errorVars = varsToBeChecked.filter((type) => configType.includes(type))

    if (errorVars && errorVars.length > 0) {
      for (const errorVar of errorVars) {
        this.error(node, `Rule is set with ${this.configOption} type [var/s: ${errorVar}]`)
      }
    }
  }

  findNamesOfElementaryTypeName(jsonObject, typeToFind) {
    const names = []

    const searchInObject = (obj) => {
      if (obj.type === typeToFind && obj.name) {
        names.push(obj.name)
      }

      for (const key in obj) {
        if (typeof obj[key] === 'object' && obj[key] !== null) {
          searchInObject(obj[key])
        }
      }
    }

    searchInObject(jsonObject)
    return names
  }
}

module.exports = ExplicitTypesChecker
