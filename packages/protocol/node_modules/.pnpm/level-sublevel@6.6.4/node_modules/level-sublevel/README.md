# level-sublevel

Separate sections of levelup, with hooks!

[![build status](https://secure.travis-ci.org/dominictarr/level-sublevel.png)](http://travis-ci.org/dominictarr/level-sublevel)

[![testling badge](https://ci.testling.com/dominictarr/level-sublevel.png)](https://ci.testling.com/dominictarr/level-sublevel)

This module allows you to create separate sections of a
[levelup](https://github.com/rvagg/node-levelup) database,
kinda like tables in an sql database, but evented, and ranged,
for real-time changing data.

## level-sublevel@6 **BREAKING CHANGES**

The long awaited `level-sublevel` rewrite is out!
You are hereby warned this is a _significant breaking_ change.
So it's good to use it with a new project,
The user api is _mostly_ the same as before,
but the way that keys are _encoded_ has changed, and _this means
you cannot run 6 on a database you created with 5_.

Also, `createWriteStream` has been removed, in anticipation of [this
change](https://github.com/rvagg/node-levelup/pull/207) use something
like [level-write-stream](https://github.com/Raynos/level-write-stream)

### Legacy Mode

Using leveldb with legacy mode is the simplest way to get the new sublevel
on top of a database that used old sublevel. Simply require sublevel like this:

``` js
var level = require('level')
                                   //  V *** require legacy.js ***
var sublevel = require('level-sublevel/legacy')
var db = sublevel(level(path))

```

### Migration Tool

@calvinmetcalf has created a migration tool:
[sublevel-migrate](https://github.com/calvinmetcalf/sublevel-migrate)

This can be used to copy an old level-sublevel into the new format.

## Stability

Unstable: Expect patches and features, possible api changes.

This module is working well, but may change in the future as its use is further explored.

## Example

``` js
var LevelUp = require('levelup')
var Sublevel = require('level-sublevel')

var db = Sublevel(LevelUp('/tmp/sublevel-example'))
var sub = db.sublevel('stuff')

//put a key into the main levelup
db.put(key, value, function () {})

//put a key into the sub-section!
sub.put(key2, value, function () {})
```

Sublevel prefixes each subsection so that it will not collide
with the outer db when saving or reading!

## Hooks

Hooks are specially built into Sublevel so that you can 
do all sorts of clever stuff, like generating views or
logs when records are inserted!

Records added via hooks will be atomically inserted with the triggering change.

### Hooks Example

Whenever a record is inserted,
save an index to it by the time it was inserted.

``` js
var sub = db.sublevel('SEQ')

db.pre(function (ch, add) {
  add({
    key: ''+Date.now(), 
    value: ch.key, 
    type: 'put',
    // NOTE: pass the destination db to add the value to that subsection!
    prefix: sub
  })
})

db.put('key', 'VALUE', function (err) {
  // read all the records inserted by the hook!
  sub.createReadStream().on('data', console.log)
})
```

Notice that the `prefix` property to `add()` is set to `sub`, which tells the hook to save the new record in the `sub` section.

## Batches

In `sublevel` batches also support a `prefix: subdb` property,
if set, this row will be inserted into that database section,
instead of the current section, similar to the `pre` hook above.

``` js
var sub1 = db.sublevel('SUB_1')
var sub2 = db.sublevel('SUM_2')

sub.batch([
  {key: 'key', value: 'Value', type: 'put'},
  {key: 'key', value: 'Value', type: 'put', prefix: sub2},
], function (err) {...})
```

## License

MIT

