semaphore.js
============

[![Build Status](https://travis-ci.org/abrkn/semaphore.js.svg?branch=master)](https://travis-ci.org/abrkn/semaphore.js)

Install:
npm install semaphore

Limit simultaneous access to a resource.

```javascript
// Create
var sem = require('semaphore')(capacity);

// Take
sem.take(fn[, n=1])
sem.take(n, fn)

// Leave
sem.leave([n])

// Available
sem.available([n])
```


```javascript
// Limit concurrent db access
var sem = require('semaphore')(1);
var server = require('http').createServer(req, res) {
	sem.take(function() {
		expensive_database_operation(function(err, res) {
			sem.leave();

			if (err) return res.end("Error");

			return res.end(res);
		});
	});
});
```

```javascript
// 2 clients at a time
var sem = require('semaphore')(2);
var server = require('http').createServer(req, res) {
	res.write("Then good day, madam!");

	sem.take(function() {
		res.end("We hope to see you soon for tea.");
		sem.leave();
	});
});
```

```javascript
// Rate limit
var sem = require('semaphore')(10);
var server = require('http').createServer(req, res) {
	sem.take(function() {
		res.end(".");
		
		setTimeout(sem.leave, 500)
	});
});
```

License
===

MIT
