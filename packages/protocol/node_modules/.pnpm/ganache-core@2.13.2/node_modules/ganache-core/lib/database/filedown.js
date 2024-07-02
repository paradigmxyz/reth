var util = require("util");
var AbstractLevelDOWN = require("abstract-leveldown").AbstractLevelDOWN;
var async = require("async");
var fs = require("fs");
var path = require("path");
var tmp = require("tmp");

util.inherits(FileDown, AbstractLevelDOWN);

function FileDown(location) {
  this.location = location;
  const tmpDir = path.join(location, "_tmp");
  try {
    // Fixes https://github.com/trufflesuite/ganache/issues/1617
    fs.mkdirSync(tmpDir, { recursive: true, mode: 0o1777 });
  } catch (e) {
    // `recursive` doesn't throw if the file exists, but `recursive` doesn't
    // exist in Node 8, so we catch the EEXISTS error in that case and ignore it.
    // once we drop node 8 suport we can remove this try/catch
    if (e.code !== "EEXIST") {
      throw e;
    }
  }
  this.tmpOptions = { keep: true, dir: tmpDir };
  AbstractLevelDOWN.call(this, location);
}

FileDown.prototype._open = function(options, callback) {
  var self = this;
  callback(null, self);
};
const accessQueue = {
  next: (lKey) => {
    const cont = accessQueue.cache[lKey].shift();
    if (cont) {
      cont();
    } else {
      delete accessQueue.cache[lKey];
    }
  },
  execute: (lKey, callback) => {
    const cache = accessQueue.cache[lKey];
    if (cache) {
      cache.push(callback);
    } else {
      accessQueue.cache[lKey] = [];
      callback();
    }
  },
  cache: {}
};

const fds = new Set();
const cleanup = (exit) => {
  try {
    fds.forEach((fdPath) => {
      const [fd, path] = fdPath;
      try {
        fs.closeSync(fd);
      } catch (e) {
        // ignore
      } finally {
        try {
          fs.unlinkSync(path);
        } catch (e) {
          // ignore
        }
      }
    });
    fds.clear();
  } finally {
    if (exit) {
      process.exit(0);
    }
  }
};
process.on("SIGINT", cleanup);
process.on("exit", () => cleanup(false));

FileDown.prototype._put = function(key, value, options, callback) {
  const lKey = path.join(this.location, key);
  // This fixes an issue caused by writing AND reading the same key multiple times
  // simultaneously. Sometimes the read operation would end up reading the value as 0 bytes
  // due to the way writes are performed in node. To fix this, we implemented a queue so only a
  // single read or write operation can occur at a time for each key; basically an in-memory lock.
  // Additionally, during testing we found that it was possible for a write operation to fail
  // due to program termination. This failure would sometimes cause the key to _exist_ but contain
  // 0 bytes (which is always invalid). To fix this we write to a temp file, and only if it works
  // do we move this temp file to its correct key location. This prevents early termination from
  // writing partial/empty values.
  // Of course, all this will eventually be for nothing as we are migrating the db to actual an
  // leveldb implementation that doesn't use a separate file for every key Soon(TM).
  accessQueue.execute(lKey, () => {
    // get a tmp file to write the contents to...
    tmp.file(this.tmpOptions, (err, path, fd, cleanupTmpFile) => {
      if (err) {
        callback(err);
        accessQueue.next(lKey);
        return;
      }
      const pair = [fd, path];
      fds.add(pair);
      const cleanupAndCallback = (err) => {
        err && cleanupTmpFile();
        fds.delete(pair);
        callback(err);
        accessQueue.next(lKey);
      };

      // write the value to our temporary file
      fs.writeFile(fd, value, "utf8", (err) => {
        if (err) {
          cleanupAndCallback(err);
          return;
        }

        // It worked! Move the temporary file to its final destination
        fs.rename(path, lKey, (err) => {
          if (err) {
            cleanupAndCallback(err);
            return;
          }

          // make sure we close this file descriptor now that the file is no
          // longer "temporary" (because we successfully moved it)
          fs.close(fd, () => cleanupAndCallback());
        });
      });
    });
  });
};

FileDown.prototype._get = function(key, options, callback) {
  const lKey = path.join(this.location, key);
  accessQueue.execute(lKey, () => {
    fs.readFile(lKey, "utf8", (err, data) => {
      if (err) {
        callback(new Error("NotFound"));
      } else {
        callback(null, data);
      }
      accessQueue.next(lKey);
    });
  });
};

FileDown.prototype._del = function(key, options, callback) {
  fs.unlink(path.join(this.location, key), function(err) {
    // Ignore when we try to delete a file that doesn't exist.
    // I'm not sure why this happens. Worth looking into.
    if (err) {
      if (err.message.indexOf("ENOENT") >= 0) {
        return callback();
      } else {
        return callback(err);
      }
    }
    callback();
  });
};

FileDown.prototype._batch = function(array, options, callback) {
  var self = this;
  async.each(
    array,
    function(item, finished) {
      if (item.type === "put") {
        self.put(item.key, item.value, options, finished);
      } else if (item.type === "del") {
        self.del(item.key, options, finished);
      } else {
        finished(new Error("Unknown batch type", item.type));
      }
    },
    function(err) {
      if (err) {
        return callback(err);
      }
      callback();
    }
  );
};

module.exports = function(location) {
  return new FileDown(location);
};
