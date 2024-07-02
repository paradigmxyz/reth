module.exports = {
  // Works the same as async.parallel
  parallel: function(fns, done) {
    done = done || function() {};
    this.map(fns, function(fn, callback) {
      fn(callback);
    }, done);
  },

  // Works the same as async.map
  map: function(items, iterator, done) {
    done = done || function() {};
    var results = [];
    var failure = false;
    var expected = items.length;
    var actual = 0;
    var createIntermediary = function(index) {
      return function(err, result) {
        // Return if we found a failure anywhere.
        // We can't stop execution of functions since they've already
        // been fired off; but we can prevent excessive handling of callbacks.
        if (failure != false) {
          return;
        }

        if (err != null) {
          failure = true;
          done(err, result);
          return;
        }

        actual += 1;

        if (actual == expected) {
          done(null, results);
        }
      };
    };

    for (var i = 0; i < items.length; i++) {
      var item = items[i];
      iterator(item, createIntermediary(i));
    }

    if (items.length == 0) {
      done(null, []);
    }
  },

  // Works like async.eachSeries
  eachSeries: function(items, iterator, done) {
    done = done || function() {};
    var results = [];
    var failure = false;
    var expected = items.length;
    var current = -1;

    function callback(err, result) {
      if (err) return done(err);

      results.push(result);

      if (current == expected) {
        return done(null, results);
      } else {
        next();
      }
    }

    function next() {
      current += 1;

      var item = items[current];
      iterator(item, callback);
    }

    next()
  }
};
