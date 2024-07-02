### Stoplight

A simple flow control mechanism.

Has two modes: "go" and "stop".

Starts stopped.

```js
var stoplight = new Stoplight()

stoplight.await(function(){
  // this will called when the stoplight is set to "go"
  // if its already "go", it will be called on the next frame
})

// starts stopped
stoplight.go()
```


### Example

Here is a class that has some async intialization process,
but can have its asynchronous method called immediately w/o breaking.

```js

function MyClass() {
  var self = this
  self._stoplight = new Stoplight()
  asyncInitialization(function(){
    self._stoplight.go()
  })
}

MyClass.prototype.asyncMethod = function(cb){
  var self = this
  self._stoplight.await(function(){
    // handle the method here and you can be sure that
    // the async initialization has finished
  })
}
```