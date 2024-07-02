function curry2(fn) {
  return function (nodeType, ...args) {
    if (args.length === 0) {
      return node => fn(nodeType, node);
    } else {
      return fn(nodeType, ...args);
    }
  };
}

module.exports.isNodeType = curry2(require('./utils/is-node-type').isNodeType);
module.exports.findAll = curry2(require('./utils/find-all').findAll);

const { astDereferencer, ASTDereferencerError } = require('./dist/ast-dereferencer');
module.exports.astDereferencer = astDereferencer;
module.exports.ASTDereferencerError = ASTDereferencerError;

const { srcDecoder } = require('./dist/src-decoder');
module.exports.srcDecoder = srcDecoder;
