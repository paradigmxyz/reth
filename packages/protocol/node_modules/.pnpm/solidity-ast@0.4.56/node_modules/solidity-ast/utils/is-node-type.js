function isNodeType(nodeType, node) {
  return nodeType === node.nodeType ||
    (nodeType === "*" && node.nodeType != undefined) ||
    (Array.isArray(nodeType) && nodeType.includes(node.nodeType));
}

module.exports = {
  isNodeType,
};
