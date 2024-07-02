# pull-defer

create a placeholder for a pull stream that won't start moving until later.

## examples

`pull-defer` can be used with source, sinks and transform streams.

### through

``` js
//create a deferred through stream
var deferred = require('pull-defer').through()

pull(
  input,
  deferred,
  output
)

//nothing will happen until deferred.resolve(stream) is called.
deferred.resolve(transform)
```

### source

``` js
//create a deferred through stream
var deferred = require('pull-defer').source()

pull(
  deferred,
  output
)

//nothing will happen until deferred.resolve(stream) is called.
deferred.resolve(input)
```

### sink

``` js
//create a deferred through stream
var deferred = require('pull-defer').sink()

pull(
  input,
  deferred
)

//nothing will happen until deferred.start(stream) is called.
deferred.resolve(output)
```


## License

MIT
