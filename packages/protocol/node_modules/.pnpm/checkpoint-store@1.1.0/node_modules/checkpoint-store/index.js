const Tree = require('functional-red-black-tree')

module.exports = CheckpointStore


function CheckpointStore(initState) {
  this._tree = Tree()
  this._checkpoints = []

  // intialize state
  initState = initState || {}
  for (var key in initState) {
    var value = initState[key]
    this.put(key, value)
  }
}

CheckpointStore.prototype.isCheckpointed = function() {
  return !!this._checkpoints.length
}

CheckpointStore.prototype.put = function(key, value) {
  var iterator = this._tree.find(key)
  if (iterator.node) {
    this._tree = iterator.update(value)
  } else {
    this._tree = this._tree.insert(key, value)
  }
}

CheckpointStore.prototype.get = function(key) {
  var iterator = this._tree.find(key)
  if (iterator.node) {
    return iterator.value
  }
}

CheckpointStore.prototype.del = function(key) {
  this._tree = this._tree.remove(key)
}

CheckpointStore.prototype.checkpoint = function() {
  this._checkpoints.push(this._tree)
}

CheckpointStore.prototype.revert = function() {
  if (this.isCheckpointed()) {
    this._tree = this._checkpoints.pop()
  } else {
    throw new Error('Checkpoint store reverted without a checkpoint.')
  }
}

CheckpointStore.prototype.commit = function() {
  if (this.isCheckpointed()) {
    this._checkpoints.pop()
  } else {
    throw new Error('Checkpoint store committed without a checkpoint.')
  }
}

CheckpointStore.prototype.copy = function() {
  var copy = new CheckpointStore()
  copy._tree = this._tree
  copy.checkpoints = this.checkpoints.slice()
  return copy
}

CheckpointStore.prototype.toJSON = function() {
  var self = this
  var data = {}
  self._tree.keys.forEach(function(key){
    var value = self.get(key)
    data[key] = value
  })
  return data
}