class TreeTraversing {
  statementNotContains(node, type) {
    const statement = this.findParentStatement(node)

    if (!statement) {
      return false
    }

    const itemOfType = this.findDownType(statement, type)

    return itemOfType !== null
  }

  findParentStatement(node) {
    while (node.parent != null && !node.parent.type.includes('Statement')) {
      node = node.parent
    }

    return node.parent
  }

  findParentType(node, type) {
    while (node.parent !== undefined && node.parent.type !== type) {
      node = node.parent
    }

    return node.parent || null
  }

  findDownType(node, type) {
    if (!node || node.type === type) {
      return node
    } else {
      return null
    }
  }

  /**
   * Traverses the tree up and checks `predicate` in each node.
   *
   * @returns {boolean}
   */
  someParent(node, predicate) {
    let parent = node.parent
    while (parent) {
      if (predicate(parent)) {
        return true
      }
      parent = parent.parent
    }

    return false
  }

  *findIdentifier(ctx) {
    const children = ctx.children

    for (let i = 0; i < children.length; i += 1) {
      if (children[i].constructor.name === 'IdentifierContext') {
        yield children[i]
      }
    }

    return null
  }
}

TreeTraversing.typeOf = function typeOf(ctx) {
  if (!ctx) {
    return ''
  }

  const className = ctx.constructor.name
  const typeName = className.replace('Context', '')
  return typeName[0].toLowerCase() + typeName.substring(1)
}

TreeTraversing.hasMethodCalls = function hasMethodCalls(node, methodNames) {
  const text = node.memberName
  return methodNames.includes(text)
}

TreeTraversing.findPropertyInParents = function findPropertyInParents(node, property) {
  let curNode = node

  while (curNode !== undefined && !curNode[property]) {
    curNode = curNode.parent
  }

  return curNode && curNode[property]
}

module.exports = TreeTraversing
