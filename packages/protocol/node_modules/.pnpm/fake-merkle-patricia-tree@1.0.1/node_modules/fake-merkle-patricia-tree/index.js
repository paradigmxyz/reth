const CheckpointStore = require('checkpoint-store')
const ZERO_ROOT = '0000000000000000000000000000000000000000000000000000000000000000'

module.exports = FakeTree


function FakeTree(initState) {
  var self = this
  self._tree = new CheckpointStore(initState)
  self.root = new Buffer(ZERO_ROOT, 'hex')
}

FakeTree.prototype.get = function(key, cb){
  var self = this
  var keyHex = key.toString('hex')
  var value = self._tree.get(keyHex)
  callAsync(cb, null, value)
}

FakeTree.prototype.put = function(key, value, cb){
  var self = this
  var keyHex = key.toString('hex')
  self._tree.put(keyHex, value)
  callAsync(cb)
}

FakeTree.prototype.del = function(key, cb){
  var self = this
  var keyHex = key.toString('hex')
  self._tree.del(keyHex)
  callAsync(cb)
}

FakeTree.prototype.checkpoint = function(){
  var self = this
  self._tree.checkpoint()
}

FakeTree.prototype.commit = function(cb){
  var self = this
  try {
    self._tree.commit()
    callAsync(cb)
  } catch (err) {
    callAsync(cb, err)
  }
}

FakeTree.prototype.revert = function(cb){
  var self = this
  try {
    self._tree.commit()
    callAsync(cb)
  } catch (err) {
    callAsync(cb, err)
  }
}

FakeTree.prototype.copy = function(){
  var self = this
  var copy = new FakeTree()
  copy._tree = self._tree.copy()
  return copy
}

FakeTree.prototype.putRaw = function(key, value, cb){
  var self = this
  self._tree.put(key, value)
  callAsync(cb)
}

FakeTree.prototype.getRaw = function(key, cb){
  var self = this
  var value = self._tree.get(key)
  callAsync(cb, null, value)
}

FakeTree.prototype.delRaw = function(key, cb){
  var self = this
  self._tree.del(key)
  callAsync(cb)
}

FakeTree.prototype.batch = function(ops, cb){
  throw new Error('FakeTree - "batch" not implemented')
}

FakeTree.prototype.createReadStream = function(){
  throw new Error('FakeTree - "createReadStream" not implemented')
}

// util

function callAsync(fn, a, b){
  setTimeout(fn.bind(null, a, b))
}