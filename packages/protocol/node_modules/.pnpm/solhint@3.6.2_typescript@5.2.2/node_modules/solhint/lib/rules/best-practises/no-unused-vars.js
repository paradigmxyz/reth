const _ = require('lodash')
const BaseChecker = require('../base-checker')
const TreeTraversing = require('../../common/tree-traversing')

const traversing = new TreeTraversing()

const ruleId = 'no-unused-vars'
const meta = {
  type: 'best-practises',

  docs: {
    description: 'Variable "name" is unused.',
    category: 'Best Practise Rules',
  },

  isDefault: false,
  recommended: true,
  defaultSetup: 'warn',

  schema: null,
}

class NoUnusedVarsChecker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  FunctionDefinition(node) {
    const funcWithoutBlock = isFuncWithoutBlock(node)
    const emptyBlock = isEmptyBlock(node)

    if (!ignoreWhen(funcWithoutBlock, emptyBlock)) {
      VarUsageScope.activate(node)
      // filters 'payable' keyword as name when defining function like this:
      // function foo(address payable) public view {}
      node.parameters
        .filter((parameter) => {
          if (
            parameter.typeName.name === 'address' &&
            parameter.typeName.stateMutability === null &&
            parameter.name === 'payable'
          )
            return null
          else return parameter.name
        })

        .forEach((parameter) => {
          this._addVariable(parameter)
        })
    }
  }

  VariableDeclarationStatement(node) {
    node.variables.forEach((variable) => this._addVariable(variable))
  }

  AssemblyLocalDefinition(node) {
    node.names.forEach((variable) => this._addVariable(variable))
  }

  Identifier(node) {
    this._trackVarUsage(node)
  }

  AssemblyCall(node) {
    const firstChild = node.arguments.length === 0 && node

    if (firstChild) {
      firstChild.name = firstChild.functionName
      this._trackVarUsage(firstChild)
    }
  }

  'FunctionDefinition:exit'(node) {
    if (VarUsageScope.isActivated(node)) {
      this._reportErrorsFor(node)
    }
  }

  _addVariable(node) {
    const idNode = node
    const funcScope = VarUsageScope.of(node)

    if (idNode && funcScope) {
      funcScope.addVar(idNode, idNode.name)
    }
  }

  _trackVarUsage(node) {
    const isFunctionName = node.type === 'FunctionDefinition'
    const funcScope = VarUsageScope.of(node)

    if (funcScope && !this._isVarDeclaration(node) && !isFunctionName) {
      funcScope.trackVarUsage(node.name)
    }
  }

  _reportErrorsFor(node) {
    VarUsageScope.of(node).unusedVariables().forEach(this._error.bind(this))
  }

  _error({ name, node }) {
    this.warn(node, `Variable "${name}" is unused`)
  }

  _isVarDeclaration(node) {
    const variableDeclaration = findParentType(node, 'VariableDeclaration')
    const identifierList = findParentType(node, 'IdentifierList')
    const parameterList = findParentType(node, 'ParameterList')
    const assemblyLocalDefinition = findParentType(node, 'AssemblyLocalDefinition')

    // otherwise `let t := a` doesn't track usage of `a`
    const isAssemblyLocalDefinition =
      assemblyLocalDefinition &&
      assemblyLocalDefinition.names &&
      assemblyLocalDefinition.names.includes(node)

    return variableDeclaration || identifierList || parameterList || isAssemblyLocalDefinition
  }
}

class VarUsageScope {
  static of(node) {
    let functionNode

    if (node.type === 'FunctionDefinition') {
      functionNode = node
    } else {
      functionNode = findParentType(node, 'FunctionDefinition')
    }

    return functionNode && functionNode.funcScope
  }

  static activate(node) {
    node.funcScope = new VarUsageScope()
  }

  static isActivated(node) {
    return !!node.funcScope
  }

  constructor() {
    this.vars = {}
  }

  addVar(node, name) {
    this.vars[name] = { node, usage: 0 }
  }

  trackVarUsage(name) {
    const curVar = this.vars[name]

    if (curVar) {
      curVar.usage += 1
    }
  }

  unusedVariables() {
    return _(this.vars)
      .pickBy((val) => val.usage === 0)
      .map((info, varName) => ({ name: varName, node: info.node }))
      .value()
  }
}

function isEmptyBlock(node) {
  return _.size(node.body && node.body.statements) === 0
}

function isFuncWithoutBlock(node) {
  return node.body === null
}

function ignoreWhen(...args) {
  return _.some(args)
}

function findParentType(node, type) {
  return traversing.findParentType(node, type)
}

module.exports = NoUnusedVarsChecker
