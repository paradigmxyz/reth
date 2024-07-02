# pull-live

construct a pull-stream for reading from a writable source,
can read old records, new (live) records, or both.

to be used by [pull-level](https://github.com/pull-stream/pull-level), 
[multiblobs](https://github.com/dominictarr/multiblob), and
[secure-scuttlebutt](https://github.com/ssbc/secure-scuttlebutt).
`pull-live` is generic, and easy to adapt to a new case.

## api: createLive(createSource(opts), createLive(opts)) => createLiveStream(opts)


createLive takes two functions, `createSource` (which returns a source
stream of the stored data) and `createLive` which returns a stream
of the live data. A function that takes `opts` and is returned.

if `opts.live` is set to true, the stream will only read the old data
(from `createSource`) and then the new data (from `createLive`) with
one item `{sync: true}` to mark when the old data has finished.

If `opts.sync === false` then the sync item will dropped.

if `opts.live` is  true (default: `false`) the live data is included.
if `opts.old` is false (default: `true`) the output will not include
the old data. If `live` and `old` are both false, an error is thrown.

the only valid options are `{live: true, old: false}` `{live: false, old: true}`
and `{live: true, old: true}`

I recomment using [pull-notify](https://github.com/pull-stream/pull-notify)
to implement `createLive`.

``` js
var MyLiveStream = createLive(createSource, createLive)

pull(MyLiveStrea({live:..., old:...}),...)
```


## License

MIT




