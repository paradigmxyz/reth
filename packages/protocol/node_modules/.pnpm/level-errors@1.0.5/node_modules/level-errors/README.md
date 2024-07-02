
# level-errors

<img alt="LevelDB Logo" height="100" src="http://leveldb.org/img/logo.svg">

**Error module for [LevelUP](https://github.com/rvagg/node-levelup)**

[![Build Status](https://travis-ci.org/Level/errors.png)](https://travis-ci.org/Level/errors) [![Greenkeeper badge](https://badges.greenkeeper.io/Level/errors.svg)](https://greenkeeper.io/)

## Usage

```js
var levelup = require('levelup')
var errors = levelup.errors

levelup('./db', { createIfMissing: false }, function (err, db) {
  if (err instanceof errors.OpenError) {
    console.log('open failed because expected db to exist')
  }
})
```

## API

### .LevelUPError()

  Generic error base class.

### .InitializationError()

  Error initializing the database, like when the database's location argument is missing.

### .OpenError()

  Error opening the database.

### .ReadError()

  Error reading from the database.

### .WriteError()

  Error writing to the database.

### .NotFoundError()

  Data not found error.

  Has extra properties:

  - `notFound`: `true`
  - `status`: 404

### .EncodingError()

  Error encoding data.

## Publishers

* [@ralphtheninja](https://github.com/ralphtheninja)
* [@juliangruber](https://github.com/juliangruber)

## License &amp; copyright

Copyright (c) 2012-2017 LevelUP contributors.

LevelUP is licensed under the MIT license. All rights not explicitly granted in the MIT license are reserved. See the included LICENSE.md file for more details.
