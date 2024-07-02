"use strict";

var ethUtil = require('ethereumjs-util');

module.exports = secureInterface;

function secureInterface(trie) {
  // overwrites
  trie.copy = copy.bind(trie, trie.copy.bind(trie));
  trie.get = get.bind(trie, trie.get.bind(trie));
  trie.put = put.bind(trie, trie.put.bind(trie));
  trie.del = del.bind(trie, trie.del.bind(trie));
} // adds the interface when copying the trie


function copy(_super) {
  var trie = _super();

  secureInterface(trie);
  return trie;
}

function get(_super, key, cb) {
  var hash = ethUtil.sha3(key);

  _super(hash, cb);
} // for a falsey value, use the original key
// to avoid double hashing the key


function put(_super, key, val, cb) {
  if (!val) {
    this.del(key, cb);
  } else {
    var hash = ethUtil.sha3(key);

    _super(hash, val, cb);
  }
}

function del(_super, key, cb) {
  var hash = ethUtil.sha3(key);

  _super(hash, cb);
}