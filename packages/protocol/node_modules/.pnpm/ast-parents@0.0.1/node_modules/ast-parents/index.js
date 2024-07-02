module.exports = addParents

function addParents(ast, key) {
  walk(ast, key || 'parent')
  return ast
}

function walk(node, keyname, parent) {
  if (parent) Object.defineProperty(node, keyname, {
      value: parent
    , configurable: true
    , enumerable: false
    , writable: true
  })

  for (var key in node) {
    if (key === 'parent') continue
    if (!node.hasOwnProperty(key)) continue

    var child = node[key]
    if (Array.isArray(child)) {
      var l = child.length

      for (var i = 0; i < l; i++) {
        if (child[i] && child[i].type)
          walk(child[i], keyname, node)
      }
    } else
    if (child && child.type) {
      walk(child, keyname, node)
    }
  }
}
