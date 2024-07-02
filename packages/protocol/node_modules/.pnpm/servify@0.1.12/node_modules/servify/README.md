## Servify

Microservices the simplest way conceivable.

## Usage

### Create a microservice:

```javascript
const servify = require("servify");

// The service state
let count = 0;

// Starts a microservice with 3 API methods
servify.api(3000, {

    // Squares a number
    square: (x) => x * x,

    // Concats two arrays
    concat: (a, b) => a.concat(b),

    // Increments and returns the counter
    count: () => count++

}).then(() => console.log("servified port 3000"))
```

### Call a microservice from code:

```javascript
const servify = require("servify");

// Builds the API interface from an URL
const api = servify.at("http://localhost:3000");

// Calls API methods like normal lib functions
api.square(2)
    .then(x => console.log(x));

api.concat([1,2], [3,4])
    .then(arr => console.log(arr));

api.count()
    .then(i => console.log(i));
```

### Call a microservice from the browser / request:

```javascript
Just access the url followed by a function call:

http://localhost:3000/square(2)
http://localhost:3000/concat([1,2], [3,4])
http://localhost:3000/count()
```

## Support

This requires ES6 Proxy support, so you need node.js 6 and up. Proxies cannot be polyfilled in earlier versions.

## Why

When all you want is to create a microservice, [Express.js](http://expressjs.com) becomes annoyingly verbose. You have to worry about things like serializing/deserializing JSON, chosing how to format query/param inputs, picking a XHR lib on the client and so on. Servify is a ridiculously thin (50 LOC) lib that just standardizes that boring stuff. To create a microservice, all you need is an object of functions specifying your API. To interact with a service, all you need is its URL. You can then call its functions exactly like you would call a normal lib (except it returns a Promise, obviously).
