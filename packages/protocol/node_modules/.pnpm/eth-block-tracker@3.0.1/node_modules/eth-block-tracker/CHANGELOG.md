### 3.0.0

- npm module main now exports unprocessed source
- module includes dist:
  - bundle: `dist/EthBlockTracker.js`
  - es5 source: `dist/es5/`
- fixes `awaitCurrentBlock` return value
- `lib` renamed to `src`
- `eth-block-tracker` is now a normal `EventEmitter`, does not provide a callback to event handlers
