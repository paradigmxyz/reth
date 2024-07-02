# stream-to-pull-stream

Convert a classic-stream, or a new-stream into a
[pull-stream](https://github.com/dominictarr/pull-stream)

## example

``` js
var toPull = require('stream-to-pull-stream')
var pull = require('pull-stream')

pull(
  toPull.source(fs.createReadStream(__filename)),
  pull.map(function (e) { return e.toString().toUpperCase() }),
  toPull.sink(process.stdout, function (err) {
    if(err) throw err
    console.log('done')
  })
)
```

if the node steam is a duplex (i.e. net, ws) then use `toPull.duplex(stream, cb?)`
`duplex` takes an optional callback in the same way that `sink` does.

## License

MIT
