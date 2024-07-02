# safe-event-emitter

An `EventEmitter` that isolates the emitter from errors in handlers. If an error is thrown in a handler it is caught and re-thrown inside of a `setTimeout` so as to not interupt the emitter's code flow.

API is the same as `EventEmitter`.

### usage

```js
const SafeEventEmitter = require('safe-event-emitter')

const ee = new SafeEventEmitter()
ee.on('boom', () => { throw new Error() })
ee.emit('boom') // no error here

// error is thrown after setTimeout
```
