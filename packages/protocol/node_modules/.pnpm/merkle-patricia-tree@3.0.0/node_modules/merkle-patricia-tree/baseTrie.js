"use strict";

function _classCallCheck(instance, Constructor) { if (!(instance instanceof Constructor)) { throw new TypeError("Cannot call a class as a function"); } }

function _defineProperties(target, props) { for (var i = 0; i < props.length; i++) { var descriptor = props[i]; descriptor.enumerable = descriptor.enumerable || false; descriptor.configurable = true; if ("value" in descriptor) descriptor.writable = true; Object.defineProperty(target, descriptor.key, descriptor); } }

function _createClass(Constructor, protoProps, staticProps) { if (protoProps) _defineProperties(Constructor.prototype, protoProps); if (staticProps) _defineProperties(Constructor, staticProps); return Constructor; }

var assert = require('assert');

var level = require('level-mem');

var async = require('async');

var rlp = require('rlp');

var ethUtil = require('ethereumjs-util');

var semaphore = require('semaphore');

var TrieNode = require('./trieNode');

var ReadStream = require('./readStream');

var PrioritizedTaskExecutor = require('./prioritizedTaskExecutor');

var matchingNibbleLength = require('./util').matchingNibbleLength;

var doKeysMatch = require('./util').doKeysMatch;

var callTogether = require('./util').callTogether;

var asyncFirstSeries = require('./util').asyncFirstSeries;
/**
 * Use `require('merkel-patricia-tree')` for the base interface. In Ethereum applications stick with the Secure Trie Overlay `require('merkel-patricia-tree/secure')`. The API for the raw and the secure interface are about the same
 * @class Trie
 * @public
 * @param {Object} [db] An instance of [levelup](https://github.com/rvagg/node-levelup/) or a compatible API. If the db is `null` or left undefined, then the trie will be stored in memory via [memdown](https://github.com/rvagg/memdown)
 * @param {Buffer|String} [root] A hex `String` or `Buffer` for the root of a previously stored trie
 * @prop {Buffer} root The current root of the `trie`
 * @prop {Boolean} isCheckpoint  determines if you are saving to a checkpoint or directly to the db
 * @prop {Buffer} EMPTY_TRIE_ROOT the Root for an empty trie
 */


module.exports =
/*#__PURE__*/
function () {
  function Trie(db, root) {
    _classCallCheck(this, Trie);

    var self = this;
    this.EMPTY_TRIE_ROOT = ethUtil.SHA3_RLP;
    this.sem = semaphore(1); // setup dbs

    this.db = db || level();
    this._getDBs = [this.db];
    this._putDBs = [this.db];
    Object.defineProperty(this, 'root', {
      set: function set(value) {
        if (value) {
          value = ethUtil.toBuffer(value);
          assert(value.length === 32, 'Invalid root length. Roots are 32 bytes');
        } else {
          value = self.EMPTY_TRIE_ROOT;
        }

        this._root = value;
      },
      get: function get() {
        return this._root;
      }
    });
    this.root = root;
    this.putRaw = this._putRaw;
  }
  /**
   * Gets a value given a `key`
   * @method get
   * @memberof Trie
   * @param {Buffer|String} key - the key to search for
   * @param {Function} cb A callback `Function` which is given the arguments `err` - for errors that may have occured and `value` - the found value in a `Buffer` or if no value was found `null`
   */


  _createClass(Trie, [{
    key: "get",
    value: function get(key, cb) {
      key = ethUtil.toBuffer(key);
      this.findPath(key, function (err, node, remainder, stack) {
        var value = null;

        if (node && remainder.length === 0) {
          value = node.value;
        }

        cb(err, value);
      });
    }
    /**
     * Stores a given `value` at the given `key`
     * @method put
     * @memberof Trie
     * @param {Buffer|String} key
     * @param {Buffer|String} Value
     * @param {Function} cb A callback `Function` which is given the argument `err` - for errors that may have occured
     */

  }, {
    key: "put",
    value: function put(key, value, cb) {
      var _this = this;

      key = ethUtil.toBuffer(key);
      value = ethUtil.toBuffer(value);

      if (!value || value.toString() === '') {
        this.del(key, cb);
      } else {
        cb = callTogether(cb, this.sem.leave);
        this.sem.take(function () {
          if (_this.root.toString('hex') !== ethUtil.SHA3_RLP.toString('hex')) {
            // first try to find the give key or its nearst node
            _this.findPath(key, function (err, foundValue, keyRemainder, stack) {
              if (err) {
                return cb(err);
              } // then update


              _this._updateNode(key, value, keyRemainder, stack, cb);
            });
          } else {
            _this._createInitialNode(key, value, cb); // if no root initialize this trie

          }
        });
      }
    }
    /**
     * deletes a value given a `key`
     * @method del
     * @memberof Trie
     * @param {Buffer|String} key
     * @param {Function} callback the callback `Function`
     */

  }, {
    key: "del",
    value: function del(key, cb) {
      var _this2 = this;

      key = ethUtil.toBuffer(key);
      cb = callTogether(cb, this.sem.leave);
      this.sem.take(function () {
        _this2.findPath(key, function (err, foundValue, keyRemainder, stack) {
          if (err) {
            return cb(err);
          }

          if (foundValue) {
            _this2._deleteNode(key, stack, cb);
          } else {
            cb();
          }
        });
      });
    }
    /**
     * Retrieves a raw value in the underlying db
     * @method getRaw
     * @memberof Trie
     * @param {Buffer} key
     * @param {Function} callback A callback `Function`, which is given the arguments `err` - for errors that may have occured and `value` - the found value in a `Buffer` or if no value was found `null`.
     */

  }, {
    key: "getRaw",
    value: function getRaw(key, cb) {
      key = ethUtil.toBuffer(key);

      function dbGet(db, cb2) {
        db.get(key, {
          keyEncoding: 'binary',
          valueEncoding: 'binary'
        }, function (err, foundNode) {
          if (err || !foundNode) {
            cb2(null, null);
          } else {
            cb2(null, foundNode);
          }
        });
      }

      asyncFirstSeries(this._getDBs, dbGet, cb);
    } // retrieves a node from dbs by hash

  }, {
    key: "_lookupNode",
    value: function _lookupNode(node, cb) {
      if (TrieNode.isRawNode(node)) {
        cb(new TrieNode(node));
      } else {
        this.getRaw(node, function (err, value) {
          if (err) {
            throw err;
          }

          if (value) {
            value = new TrieNode(rlp.decode(value));
          }

          cb(value);
        });
      }
    }
    /**
     * Writes a value directly to the underlining db
     * @method putRaw
     * @memberof Trie
     * @param {Buffer|String} key The key as a `Buffer` or `String`
     * @param {Buffer} value The value to be stored
     * @param {Function} callback A callback `Function`, which is given the argument `err` - for errors that may have occured
     */
    // TODO: remove the proxy method when changing the caching

  }, {
    key: "_putRaw",
    value: function _putRaw(key, val, cb) {
      function dbPut(db, cb2) {
        db.put(key, val, {
          keyEncoding: 'binary',
          valueEncoding: 'binary'
        }, cb2);
      }

      async.each(this._putDBs, dbPut, cb);
    }
    /**
     * Removes a raw value in the underlying db
     * @method delRaw
     * @memberof Trie
     * @param {Buffer|String} key
     * @param {Function} callback A callback `Function`, which is given the argument `err` - for errors that may have occured
     */

  }, {
    key: "delRaw",
    value: function delRaw(key, cb) {
      function del(db, cb2) {
        db.del(key, {
          keyEncoding: 'binary'
        }, cb2);
      }

      async.each(this._putDBs, del, cb);
    } // writes a single node to dbs

  }, {
    key: "_putNode",
    value: function _putNode(node, cb) {
      var hash = node.hash();
      var serialized = node.serialize();

      this._putRaw(hash, serialized, cb);
    } // writes many nodes to db

  }, {
    key: "_batchNodes",
    value: function _batchNodes(opStack, cb) {
      function dbBatch(db, cb) {
        db.batch(opStack, {
          keyEncoding: 'binary',
          valueEncoding: 'binary'
        }, cb);
      }

      async.each(this._putDBs, dbBatch, cb);
    }
    /**
     * Tries to find a path to the node for the given key
     * It returns a `stack` of nodes to the closet node
     * @method findPath
     * @memberof Trie
     * @param {String|Buffer} - key - the search key
     * @param {Function} - cb - the callback function. Its is given the following
     * arguments
     *  - err - any errors encontered
     *  - node - the last node found
     *  - keyRemainder - the remaining key nibbles not accounted for
     *  - stack - an array of nodes that forms the path to node we are searching for
     */

  }, {
    key: "findPath",
    value: function findPath(targetKey, cb) {
      var stack = [];
      targetKey = TrieNode.stringToNibbles(targetKey);

      this._walkTrie(this.root, processNode, cb);

      function processNode(nodeRef, node, keyProgress, walkController) {
        var nodeKey = node.key || [];
        var keyRemainder = targetKey.slice(matchingNibbleLength(keyProgress, targetKey));
        var matchingLen = matchingNibbleLength(keyRemainder, nodeKey);
        stack.push(node);

        if (node.type === 'branch') {
          if (keyRemainder.length === 0) {
            walkController.return(null, node, [], stack); // we exhausted the key without finding a node
          } else {
            var branchIndex = keyRemainder[0];
            var branchNode = node.getValue(branchIndex);

            if (!branchNode) {
              // there are no more nodes to find and we didn't find the key
              walkController.return(null, null, keyRemainder, stack);
            } else {
              // node found, continuing search
              walkController.only(branchIndex);
            }
          }
        } else if (node.type === 'leaf') {
          if (doKeysMatch(keyRemainder, nodeKey)) {
            // keys match, return node with empty key
            walkController.return(null, node, [], stack);
          } else {
            // reached leaf but keys dont match
            walkController.return(null, null, keyRemainder, stack);
          }
        } else if (node.type === 'extention') {
          if (matchingLen !== nodeKey.length) {
            // keys dont match, fail
            walkController.return(null, null, keyRemainder, stack);
          } else {
            // keys match, continue search
            walkController.next();
          }
        }
      }
    }
    /*
     * Finds all nodes that store k,v values
     */

  }, {
    key: "_findNode",
    value: function _findNode(key, root, stack, cb) {
      var _arguments = arguments;
      this.findPath(key, function () {
        cb.apply(null, _arguments);
      });
    }
    /*
     * Finds all nodes that store k,v values
     */

  }, {
    key: "_findValueNodes",
    value: function _findValueNodes(onFound, cb) {
      this._walkTrie(this.root, function (nodeRef, node, key, walkController) {
        var fullKey = key;

        if (node.key) {
          fullKey = key.concat(node.key);
        }

        if (node.type === 'leaf') {
          // found leaf node!
          onFound(nodeRef, node, fullKey, walkController.next);
        } else if (node.type === 'branch' && node.value) {
          // found branch with value
          onFound(nodeRef, node, fullKey, walkController.next);
        } else {
          // keep looking for value nodes
          walkController.next();
        }
      }, cb);
    }
    /*
     * Finds all nodes that are stored directly in the db
     * (some nodes are stored raw inside other nodes)
     */

  }, {
    key: "_findDbNodes",
    value: function _findDbNodes(onFound, cb) {
      this._walkTrie(this.root, function (nodeRef, node, key, walkController) {
        if (TrieNode.isRawNode(nodeRef)) {
          walkController.next();
        } else {
          onFound(nodeRef, node, key, walkController.next);
        }
      }, cb);
    }
    /**
     * Updates a node
     * @method _updateNode
     * @private
     * @param {Buffer} key
     * @param {Buffer| String} value
     * @param {Array} keyRemainder
     * @param {Array} stack -
     * @param {Function} cb - the callback
     */

  }, {
    key: "_updateNode",
    value: function _updateNode(key, value, keyRemainder, stack, cb) {
      var toSave = [];
      var lastNode = stack.pop(); // add the new nodes

      key = TrieNode.stringToNibbles(key); // Check if the last node is a leaf and the key matches to this

      var matchLeaf = false;

      if (lastNode.type === 'leaf') {
        var l = 0;

        for (var i = 0; i < stack.length; i++) {
          var n = stack[i];

          if (n.type === 'branch') {
            l++;
          } else {
            l += n.key.length;
          }
        }

        if (matchingNibbleLength(lastNode.key, key.slice(l)) === lastNode.key.length && keyRemainder.length === 0) {
          matchLeaf = true;
        }
      }

      if (matchLeaf) {
        // just updating a found value
        lastNode.value = value;
        stack.push(lastNode);
      } else if (lastNode.type === 'branch') {
        stack.push(lastNode);

        if (keyRemainder !== 0) {
          // add an extention to a branch node
          keyRemainder.shift(); // create a new leaf

          var newLeaf = new TrieNode('leaf', keyRemainder, value);
          stack.push(newLeaf);
        } else {
          lastNode.value = value;
        }
      } else {
        // create a branch node
        var lastKey = lastNode.key;
        var matchingLength = matchingNibbleLength(lastKey, keyRemainder);
        var newBranchNode = new TrieNode('branch'); // create a new extention node

        if (matchingLength !== 0) {
          var newKey = lastNode.key.slice(0, matchingLength);
          var newExtNode = new TrieNode('extention', newKey, value);
          stack.push(newExtNode);
          lastKey.splice(0, matchingLength);
          keyRemainder.splice(0, matchingLength);
        }

        stack.push(newBranchNode);

        if (lastKey.length !== 0) {
          var branchKey = lastKey.shift();

          if (lastKey.length !== 0 || lastNode.type === 'leaf') {
            // shriking extention or leaf
            lastNode.key = lastKey;

            var formatedNode = this._formatNode(lastNode, false, toSave);

            newBranchNode.setValue(branchKey, formatedNode);
          } else {
            // remove extention or attaching
            this._formatNode(lastNode, false, true, toSave);

            newBranchNode.setValue(branchKey, lastNode.value);
          }
        } else {
          newBranchNode.value = lastNode.value;
        }

        if (keyRemainder.length !== 0) {
          keyRemainder.shift(); // add a leaf node to the new branch node

          var newLeafNode = new TrieNode('leaf', keyRemainder, value);
          stack.push(newLeafNode);
        } else {
          newBranchNode.value = value;
        }
      }

      this._saveStack(key, stack, toSave, cb);
    } // walk tree

  }, {
    key: "_walkTrie",
    value: function _walkTrie(root, onNode, onDone) {
      var self = this;
      root = root || this.root;

      onDone = onDone || function () {};

      var aborted = false;
      var returnValues = [];

      if (root.toString('hex') === ethUtil.SHA3_RLP.toString('hex')) {
        return onDone();
      }

      this._lookupNode(root, function (node) {
        processNode(root, node, null, function (err) {
          if (err) {
            return onDone(err);
          }

          onDone.apply(null, returnValues);
        });
      }); // the maximum pool size should be high enough to utilise the parallelizability of reading nodes from disk and
      // low enough to utilize the prioritisation of node lookup.


      var maxPoolSize = 500;
      var taskExecutor = new PrioritizedTaskExecutor(maxPoolSize);

      function processNode(nodeRef, node, key, cb) {
        if (!node || aborted) {
          return cb();
        }

        var stopped = false;
        key = key || [];
        var walkController = {
          stop: function stop() {
            stopped = true;
            cb();
          },
          // end all traversal and return values to the onDone cb
          return: function _return() {
            aborted = true;
            returnValues = arguments;
            cb();
          },
          next: function next() {
            if (aborted || stopped) {
              return cb();
            }

            var children = node.getChildren();
            async.forEachOf(children, function (childData, index, cb) {
              var keyExtension = childData[0];
              var childRef = childData[1];
              var childKey = key.concat(keyExtension);
              var priority = childKey.length;
              taskExecutor.execute(priority, function (taskCallback) {
                self._lookupNode(childRef, function (childNode) {
                  taskCallback();
                  processNode(childRef, childNode, childKey, cb);
                });
              });
            }, cb);
          },
          only: function only(childIndex) {
            var childRef = node.getValue(childIndex);
            var childKey = key.slice();
            childKey.push(childIndex);
            var priority = childKey.length;
            taskExecutor.execute(priority, function (taskCallback) {
              self._lookupNode(childRef, function (childNode) {
                taskCallback();
                processNode(childRef, childNode, childKey, cb);
              });
            });
          }
        };
        onNode(nodeRef, node, key, walkController);
      }
    }
    /**
     * saves a stack
     * @method _saveStack
     * @private
     * @param {Array} key - the key. Should follow the stack
     * @param {Array} stack - a stack of nodes to the value given by the key
     * @param {Array} opStack - a stack of levelup operations to commit at the end of this funciton
     * @param {Function} cb
     */

  }, {
    key: "_saveStack",
    value: function _saveStack(key, stack, opStack, cb) {
      var lastRoot; // update nodes

      while (stack.length) {
        var node = stack.pop();

        if (node.type === 'leaf') {
          key.splice(key.length - node.key.length);
        } else if (node.type === 'extention') {
          key.splice(key.length - node.key.length);

          if (lastRoot) {
            node.value = lastRoot;
          }
        } else if (node.type === 'branch') {
          if (lastRoot) {
            var branchKey = key.pop();
            node.setValue(branchKey, lastRoot);
          }
        }

        lastRoot = this._formatNode(node, stack.length === 0, opStack);
      }

      if (lastRoot) {
        this.root = lastRoot;
      }

      this._batchNodes(opStack, cb);
    }
  }, {
    key: "_deleteNode",
    value: function _deleteNode(key, stack, cb) {
      var _this3 = this;

      function processBranchNode(key, branchKey, branchNode, parentNode, stack) {
        // branchNode is the node ON the branch node not THE branch node
        var branchNodeKey = branchNode.key;

        if (!parentNode || parentNode.type === 'branch') {
          // branch->?
          if (parentNode) {
            stack.push(parentNode);
          }

          if (branchNode.type === 'branch') {
            // create an extention node
            // branch->extention->branch
            var extentionNode = new TrieNode('extention', [branchKey], null);
            stack.push(extentionNode);
            key.push(branchKey);
          } else {
            // branch key is an extention or a leaf
            // branch->(leaf or extention)
            branchNodeKey.unshift(branchKey);
            branchNode.key = branchNodeKey; // hackery. This is equvilant to array.concat except we need keep the
            // rerfance to the `key` that was passed in.

            branchNodeKey.unshift(0);
            branchNodeKey.unshift(key.length);
            key.splice.apply(key, branchNodeKey);
          }

          stack.push(branchNode);
        } else {
          // parent is a extention
          var parentKey = parentNode.key;

          if (branchNode.type === 'branch') {
            // ext->branch
            parentKey.push(branchKey);
            key.push(branchKey);
            parentNode.key = parentKey;
            stack.push(parentNode);
          } else {
            // branch node is an leaf or extention and parent node is an exstention
            // add two keys together
            // dont push the parent node
            branchNodeKey.unshift(branchKey);
            key = key.concat(branchNodeKey);
            parentKey = parentKey.concat(branchNodeKey);
            branchNode.key = parentKey;
          }

          stack.push(branchNode);
        }

        return key;
      }

      var lastNode = stack.pop();
      var parentNode = stack.pop();
      var opStack = [];

      if (!Array.isArray(key)) {
        // convert key to nibbles
        key = TrieNode.stringToNibbles(key);
      }

      if (!parentNode) {
        // the root here has to be a leaf.
        this.root = this.EMPTY_TRIE_ROOT;
        cb();
      } else {
        if (lastNode.type === 'branch') {
          lastNode.value = null;
        } else {
          // the lastNode has to be a leaf if its not a branch. And a leaf's parent
          // if it has one must be a branch.
          var lastNodeKey = lastNode.key;
          key.splice(key.length - lastNodeKey.length); // delete the value

          this._formatNode(lastNode, false, true, opStack);

          parentNode.setValue(key.pop(), null);
          lastNode = parentNode;
          parentNode = stack.pop();
        } // nodes on the branch


        var branchNodes = []; // count the number of nodes on the branch

        lastNode.raw.forEach(function (node, i) {
          var val = lastNode.getValue(i);

          if (val) {
            branchNodes.push([i, val]);
          }
        }); // if there is only one branch node left, collapse the branch node

        if (branchNodes.length === 1) {
          // add the one remaing branch node to node above it
          var branchNode = branchNodes[0][1];
          var branchNodeKey = branchNodes[0][0]; // look up node

          this._lookupNode(branchNode, function (foundNode) {
            key = processBranchNode(key, branchNodeKey, foundNode, parentNode, stack, opStack);

            _this3._saveStack(key, stack, opStack, cb);
          });
        } else {
          // simple removing a leaf and recaluclation the stack
          if (parentNode) {
            stack.push(parentNode);
          }

          stack.push(lastNode);

          this._saveStack(key, stack, opStack, cb);
        }
      }
    } // Creates the initial node from an empty tree

  }, {
    key: "_createInitialNode",
    value: function _createInitialNode(key, value, cb) {
      var newNode = new TrieNode('leaf', key, value);
      this.root = newNode.hash();

      this._putNode(newNode, cb);
    } // formats node to be saved by levelup.batch.
    // returns either the hash that will be used key or the rawNode

  }, {
    key: "_formatNode",
    value: function _formatNode(node, topLevel, remove, opStack) {
      if (arguments.length === 3) {
        opStack = remove;
        remove = false;
      }

      var rlpNode = node.serialize();

      if (rlpNode.length >= 32 || topLevel) {
        var hashRoot = node.hash();

        if (remove && this.isCheckpoint) {
          opStack.push({
            type: 'del',
            key: hashRoot
          });
        } else {
          opStack.push({
            type: 'put',
            key: hashRoot,
            value: rlpNode
          });
        }

        return hashRoot;
      }

      return node.raw;
    }
    /**
     * The `data` event is given an `Object` hat has two properties; the `key` and the `value`. Both should be Buffers.
     * @method createReadStream
     * @memberof Trie
     * @return {stream.Readable} Returns a [stream](https://nodejs.org/dist/latest-v5.x/docs/api/stream.html#stream_class_stream_readable) of the contents of the `trie`
     */

  }, {
    key: "createReadStream",
    value: function createReadStream() {
      return new ReadStream(this);
    } // creates a new trie backed by the same db
    // and starting at the same root

  }, {
    key: "copy",
    value: function copy() {
      return new Trie(this.db, this.root);
    }
    /**
     * The given hash of operations (key additions or deletions) are executed on the DB
     * @method batch
     * @memberof Trie
     * @example
     * var ops = [
     *    { type: 'del', key: 'father' }
     *  , { type: 'put', key: 'name', value: 'Yuri Irsenovich Kim' }
     *  , { type: 'put', key: 'dob', value: '16 February 1941' }
     *  , { type: 'put', key: 'spouse', value: 'Kim Young-sook' }
     *  , { type: 'put', key: 'occupation', value: 'Clown' }
     * ]
     * trie.batch(ops)
     * @param {Array} ops
     * @param {Function} cb
     */

  }, {
    key: "batch",
    value: function batch(ops, cb) {
      var _this4 = this;

      async.eachSeries(ops, function (op, cb2) {
        if (op.type === 'put') {
          _this4.put(op.key, op.value, cb2);
        } else if (op.type === 'del') {
          _this4.del(op.key, cb2);
        } else {
          cb2();
        }
      }, cb);
    }
    /**
     * Checks if a given root exists
     * @method checkRoot
     * @memberof Trie
     * @param {Buffer} root
     * @param {Function} cb
     */

  }, {
    key: "checkRoot",
    value: function checkRoot(root, cb) {
      root = ethUtil.toBuffer(root);

      this._lookupNode(root, function (value) {
        cb(null, !!value);
      });
    }
  }]);

  return Trie;
}();