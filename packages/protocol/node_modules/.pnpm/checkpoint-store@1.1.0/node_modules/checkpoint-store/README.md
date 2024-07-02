# Checkpoint-Store

In-memory key-value store with history!
Keys are strings, values are any type.

```js
var store = new CheckpointStore()
store.put('xyz', 'hello world')
store.checkpoint()
store.put('xyz', 'haay wuurl')

store.get('xyz') //=> 'haay wuurl'
store.revert()
store.get('xyz') //=> 'hello world'
```

Uses [functional-red-black-tree](https://github.com/mikolalysenko/functional-red-black-tree) under the hood.

### api

##### new CheckpointStore(initialState)

creates a new store. optionally pass in the intial state as a pojo.

##### store.put(key, value)

sets a value. key is a string. value is any type.

##### store.get(key)

returns a value. key is a string. value is any type.

##### store.del(key)

deletes a value. key is a string.

##### store.checkpoint()

adds a checkpoint. state of the store can be returned to this point.

##### store.revert()

returns to the state of the previous checkpoint. throws an error if there are no checkpoints.

##### store.commit()

keeps the state and removes the previous checkpoint. throws an error if there are no checkpoints.

##### store.isCheckpointed()

returns true if the store has remaining checkpoints.

##### store.copy()

returns a new CheckpointStore with matching state and checkpoints.

##### store.toJSON()

returns an object (not a string!) of the current state.