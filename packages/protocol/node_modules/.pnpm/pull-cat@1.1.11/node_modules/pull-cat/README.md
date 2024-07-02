# pull-cat

> Concatenate pull-streams

## Install

```shell
npm install --save pull-cat
```

## Example

Construct a new source stream from a sequential list of source streams,
reading from each one in turn until it ends, then the next, etc.
If one stream errors, then the rest of the streams are aborted immediately.
If the cat stream is aborted (i.e. if it's sink errors) then all the streams
are aborted.

A cat stream is a moderately challenging stream to implement,
especially in the context of error states.

```js
var cat = require('pull-cat')
var pull = require('pull-stream')

pull(
  cat([
    pull.values([1,2,3]),
    pull.values([4,5,6])
  ]),
  pull.log()
)
// 1
// 2
// 3
// 4
// 5
// 6
```


## Api

### `cat = require('pull-cat')`

### `stream = cat(streams)`

Reads from each stream in `streams` until finished.

If a stream errors, stop all the streams.
if the concatenated stream is aborted, abort all the streams,
then callback to the aborter.

## License

MIT
