// TODO: this is a temporary fix to hide those libraries from the browser. A
// slightly better long-term solution would be to split this file into two,
// separating the functions that are used on Node.js from the functions that
// are used only on the browser.
module.exports = function (_ref) {
  var fs = _ref.fs,
      files = _ref.files,
      os = _ref.os,
      path = _ref.path,
      child_process = _ref.child_process,
      mimetype = _ref.mimetype,
      defaultArchives = _ref.defaultArchives,
      request = _ref.request,
      downloadUrl = _ref.downloadUrl,
      bytes = _ref.bytes,
      hash = _ref.hash,
      pick = _ref.pick;

  // ∀ a . String -> JSON -> Map String a -o Map String a
  //   Inserts a key/val pair in an object impurely.
  var impureInsert = function impureInsert(key) {
    return function (val) {
      return function (map) {
        return map[key] = val, map;
      };
    };
  }; // String -> JSON -> Map String JSON
  //   Merges an array of keys and an array of vals into an object.


  var toMap = function toMap(keys) {
    return function (vals) {
      var map = {};

      for (var i = 0, l = keys.length; i < l; ++i) {
        map[keys[i]] = vals[i];
      }

      return map;
    };
  }; // ∀ a . Map String a -> Map String a -> Map String a
  //   Merges two maps into one.


  var merge = function merge(a) {
    return function (b) {
      var map = {};

      for (var key in a) {
        map[key] = a[key];
      }

      for (var _key in b) {
        map[_key] = b[_key];
      }

      return map;
    };
  }; // ∀ a . [a] -> [a] -> Bool


  var equals = function equals(a) {
    return function (b) {
      if (a.length !== b.length) {
        return false;
      } else {
        for (var i = 0, l = a.length; i < l; ++i) {
          if (a[i] !== b[i]) return false;
        }
      }

      return true;
    };
  }; // String -> String -> String


  var rawUrl = function rawUrl(swarmUrl) {
    return function (hash) {
      return "".concat(swarmUrl, "/bzz-raw:/").concat(hash);
    };
  }; // String -> String -> Promise Uint8Array
  //   Gets the raw contents of a Swarm hash address.


  var downloadData = function downloadData(swarmUrl) {
    return function (hash) {
      return new Promise(function (resolve, reject) {
        request(rawUrl(swarmUrl)(hash), {
          responseType: "arraybuffer"
        }, function (err, arrayBuffer, response) {
          if (err) {
            return reject(err);
          }

          if (response.statusCode >= 400) {
            return reject(new Error("Error ".concat(response.statusCode, ".")));
          }

          return resolve(new Uint8Array(arrayBuffer));
        });
      });
    };
  }; // type Entry = {"type": String, "hash": String}
  // type File = {"type": String, "data": Uint8Array}
  // String -> String -> Promise (Map String Entry)
  //   Solves the manifest of a Swarm address recursively.
  //   Returns a map from full paths to entries.


  var downloadEntries = function downloadEntries(swarmUrl) {
    return function (hash) {
      var search = function search(hash) {
        return function (path) {
          return function (routes) {
            // Formats an entry to the Swarm.js type.
            var format = function format(entry) {
              return {
                type: entry.contentType,
                hash: entry.hash
              };
            }; // To download a single entry:
            //   if type is bzz-manifest, go deeper
            //   if not, add it to the routing table


            var downloadEntry = function downloadEntry(entry) {
              if (entry.path === undefined) {
                return Promise.resolve();
              } else {
                return entry.contentType === "application/bzz-manifest+json" ? search(entry.hash)(path + entry.path)(routes) : Promise.resolve(impureInsert(path + entry.path)(format(entry))(routes));
              }
            }; // Downloads the initial manifest and then each entry.


            return downloadData(swarmUrl)(hash).then(function (text) {
              return JSON.parse(toString(text)).entries;
            }).then(function (entries) {
              return Promise.all(entries.map(downloadEntry));
            }).then(function () {
              return routes;
            });
          };
        };
      };

      return search(hash)("")({});
    };
  }; // String -> String -> Promise (Map String String)
  //   Same as `downloadEntries`, but returns only hashes (no types).


  var downloadRoutes = function downloadRoutes(swarmUrl) {
    return function (hash) {
      return downloadEntries(swarmUrl)(hash).then(function (entries) {
        return toMap(Object.keys(entries))(Object.keys(entries).map(function (route) {
          return entries[route].hash;
        }));
      });
    };
  }; // String -> String -> Promise (Map String File)
  //   Gets the entire directory tree in a Swarm address.
  //   Returns a promise mapping paths to file contents.


  var downloadDirectory = function downloadDirectory(swarmUrl) {
    return function (hash) {
      return downloadEntries(swarmUrl)(hash).then(function (entries) {
        var paths = Object.keys(entries);
        var hashs = paths.map(function (path) {
          return entries[path].hash;
        });
        var types = paths.map(function (path) {
          return entries[path].type;
        });
        var datas = hashs.map(downloadData(swarmUrl));

        var files = function files(datas) {
          return datas.map(function (data, i) {
            return {
              type: types[i],
              data: data
            };
          });
        };

        return Promise.all(datas).then(function (datas) {
          return toMap(paths)(files(datas));
        });
      });
    };
  }; // String -> String -> String -> Promise String
  //   Gets the raw contents of a Swarm hash address.
  //   Returns a promise with the downloaded file path.


  var downloadDataToDisk = function downloadDataToDisk(swarmUrl) {
    return function (hash) {
      return function (filePath) {
        return files.download(rawUrl(swarmUrl)(hash))(filePath);
      };
    };
  }; // String -> String -> String -> Promise (Map String String)
  //   Gets the entire directory tree in a Swarm address.
  //   Returns a promise mapping paths to file contents.


  var downloadDirectoryToDisk = function downloadDirectoryToDisk(swarmUrl) {
    return function (hash) {
      return function (dirPath) {
        return downloadRoutes(swarmUrl)(hash).then(function (routingTable) {
          var downloads = [];

          for (var route in routingTable) {
            if (route.length > 0) {
              var filePath = path.join(dirPath, route);
              downloads.push(downloadDataToDisk(swarmUrl)(routingTable[route])(filePath));
            }

            ;
          }

          ;
          return Promise.all(downloads).then(function () {
            return dirPath;
          });
        });
      };
    };
  }; // String -> Uint8Array -> Promise String
  //   Uploads raw data to Swarm.
  //   Returns a promise with the uploaded hash.


  var uploadData = function uploadData(swarmUrl) {
    return function (data) {
      return new Promise(function (resolve, reject) {
        var params = {
          body: typeof data === "string" ? fromString(data) : data,
          method: "POST"
        };
        request("".concat(swarmUrl, "/bzz-raw:/"), params, function (err, data) {
          if (err) {
            return reject(err);
          }

          return resolve(data);
        });
      });
    };
  }; // String -> String -> String -> File -> Promise String
  //   Uploads a file to the Swarm manifest at a given hash, under a specific
  //   route. Returns a promise containing the uploaded hash.
  //   FIXME: for some reasons Swarm-Gateways is sometimes returning
  //   error 404 (bad request), so we retry up to 3 times. Why?


  var uploadToManifest = function uploadToManifest(swarmUrl) {
    return function (hash) {
      return function (route) {
        return function (file) {
          var attempt = function attempt(n) {
            var slashRoute = route[0] === "/" ? route : "/" + route;
            var url = "".concat(swarmUrl, "/bzz:/").concat(hash).concat(slashRoute);
            var opt = {
              method: "PUT",
              headers: {
                "Content-Type": file.type
              },
              body: file.data
            };
            return new Promise(function (resolve, reject) {
              request(url, opt, function (err, data) {
                if (err) {
                  return reject(err);
                }

                if (data.indexOf("error") !== -1) {
                  return reject(data);
                }

                return resolve(data);
              });
            })["catch"](function (e) {
              return n > 0 && attempt(n - 1);
            });
          };

          return attempt(3);
        };
      };
    };
  }; // String -> {type: String, data: Uint8Array} -> Promise String


  var uploadFile = function uploadFile(swarmUrl) {
    return function (file) {
      return uploadDirectory(swarmUrl)({
        "": file
      });
    };
  }; // String -> String -> Promise String


  var uploadFileFromDisk = function uploadFileFromDisk(swarmUrl) {
    return function (filePath) {
      return fs.readFile(filePath).then(function (data) {
        return uploadFile(swarmUrl)({
          type: mimetype.lookup(filePath),
          data: data
        });
      });
    };
  }; // String -> Map String File -> Promise String
  //   Uploads a directory to Swarm. The directory is
  //   represented as a map of routes and files.
  //   A default path is encoded by having a "" route.


  var uploadDirectory = function uploadDirectory(swarmUrl) {
    return function (directory) {
      return uploadData(swarmUrl)("{}").then(function (hash) {
        var uploadRoute = function uploadRoute(route) {
          return function (hash) {
            return uploadToManifest(swarmUrl)(hash)(route)(directory[route]);
          };
        };

        var uploadToHash = function uploadToHash(hash, route) {
          return hash.then(uploadRoute(route));
        };

        return Object.keys(directory).reduce(uploadToHash, Promise.resolve(hash));
      });
    };
  }; // String -> Promise String


  var uploadDataFromDisk = function uploadDataFromDisk(swarmUrl) {
    return function (filePath) {
      return fs.readFile(filePath).then(uploadData(swarmUrl));
    };
  }; // String -> Nullable String -> String -> Promise String


  var uploadDirectoryFromDisk = function uploadDirectoryFromDisk(swarmUrl) {
    return function (defaultPath) {
      return function (dirPath) {
        return files.directoryTree(dirPath).then(function (fullPaths) {
          return Promise.all(fullPaths.map(function (path) {
            return fs.readFile(path);
          })).then(function (datas) {
            var paths = fullPaths.map(function (path) {
              return path.slice(dirPath.length);
            });
            var types = fullPaths.map(function (path) {
              return mimetype.lookup(path) || "text/plain";
            });
            return toMap(paths)(datas.map(function (data, i) {
              return {
                type: types[i],
                data: data
              };
            }));
          });
        }).then(function (directory) {
          return merge(defaultPath ? {
            "": directory[defaultPath]
          } : {})(directory);
        }).then(uploadDirectory(swarmUrl));
      };
    };
  }; // String -> UploadInfo -> Promise String
  //   Simplified multi-type upload which calls the correct
  //   one based on the type of the argument given.


  var _upload = function upload(swarmUrl) {
    return function (arg) {
      // Upload raw data from browser
      if (arg.pick === "data") {
        return pick.data().then(uploadData(swarmUrl)); // Upload a file from browser
      } else if (arg.pick === "file") {
        return pick.file().then(uploadFile(swarmUrl)); // Upload a directory from browser
      } else if (arg.pick === "directory") {
        return pick.directory().then(uploadDirectory(swarmUrl)); // Upload directory/file from disk
      } else if (arg.path) {
        switch (arg.kind) {
          case "data":
            return uploadDataFromDisk(swarmUrl)(arg.path);

          case "file":
            return uploadFileFromDisk(swarmUrl)(arg.path);

          case "directory":
            return uploadDirectoryFromDisk(swarmUrl)(arg.defaultFile)(arg.path);
        }

        ; // Upload UTF-8 string or raw data (buffer)
      } else if (arg.length || typeof arg === "string") {
        return uploadData(swarmUrl)(arg); // Upload directory with JSON
      } else if (arg instanceof Object) {
        return uploadDirectory(swarmUrl)(arg);
      }

      return Promise.reject(new Error("Bad arguments"));
    };
  }; // String -> String -> Nullable String -> Promise (String | Uint8Array | Map String Uint8Array)
  //   Simplified multi-type download which calls the correct function based on
  //   the type of the argument given, and on whether the Swwarm address has a
  //   directory or a file.


  var _download = function download(swarmUrl) {
    return function (hash) {
      return function (path) {
        return isDirectory(swarmUrl)(hash).then(function (isDir) {
          if (isDir) {
            return path ? downloadDirectoryToDisk(swarmUrl)(hash)(path) : downloadDirectory(swarmUrl)(hash);
          } else {
            return path ? downloadDataToDisk(swarmUrl)(hash)(path) : downloadData(swarmUrl)(hash);
          }
        });
      };
    };
  }; // String -> Promise String
  //   Downloads the Swarm binaries into a path. Returns a promise that only
  //   resolves when the exact Swarm file is there, and verified to be correct.
  //   If it was already there to begin with, skips the download.


  var downloadBinary = function downloadBinary(path, archives) {
    var system = os.platform().replace("win32", "windows") + "-" + (os.arch() === "x64" ? "amd64" : "386");
    var archive = (archives || defaultArchives)[system];
    var archiveUrl = downloadUrl + archive.archive + ".tar.gz";
    var archiveMD5 = archive.archiveMD5;
    var binaryMD5 = archive.binaryMD5;
    return files.safeDownloadArchived(archiveUrl)(archiveMD5)(binaryMD5)(path);
  }; // type SwarmSetup = {
  //   account : String,
  //   password : String,
  //   dataDir : String,
  //   binPath : String,
  //   ensApi : String,
  //   onDownloadProgress : Number ~> (),
  //   archives : [{
  //     archive: String,
  //     binaryMD5: String,
  //     archiveMD5: String
  //   }]
  // }
  // SwarmSetup ~> Promise Process
  //   Starts the Swarm process.


  var startProcess = function startProcess(swarmSetup) {
    return new Promise(function (resolve, reject) {
      var spawn = child_process.spawn;

      var hasString = function hasString(str) {
        return function (buffer) {
          return ('' + buffer).indexOf(str) !== -1;
        };
      };

      var account = swarmSetup.account,
          password = swarmSetup.password,
          dataDir = swarmSetup.dataDir,
          ensApi = swarmSetup.ensApi,
          privateKey = swarmSetup.privateKey;
      var STARTUP_TIMEOUT_SECS = 3;
      var WAITING_PASSWORD = 0;
      var STARTING = 1;
      var LISTENING = 2;
      var PASSWORD_PROMPT_HOOK = "Passphrase";
      var LISTENING_HOOK = "Swarm http proxy started";
      var state = WAITING_PASSWORD;
      var swarmProcess = spawn(swarmSetup.binPath, ['--bzzaccount', account || privateKey, '--datadir', dataDir, '--ens-api', ensApi]);

      var handleProcessOutput = function handleProcessOutput(data) {
        if (state === WAITING_PASSWORD && hasString(PASSWORD_PROMPT_HOOK)(data)) {
          setTimeout(function () {
            state = STARTING;
            swarmProcess.stdin.write(password + '\n');
          }, 500);
        } else if (hasString(LISTENING_HOOK)(data)) {
          state = LISTENING;
          clearTimeout(timeout);
          resolve(swarmProcess);
        }
      };

      swarmProcess.stdout.on('data', handleProcessOutput);
      swarmProcess.stderr.on('data', handleProcessOutput); //swarmProcess.on('close', () => setTimeout(restart, 2000));

      var restart = function restart() {
        return startProcess(swarmSetup).then(resolve)["catch"](reject);
      };

      var error = function error() {
        return reject(new Error("Couldn't start swarm process."));
      };

      var timeout = setTimeout(error, 20000);
    });
  }; // Process ~> Promise ()
  //   Stops the Swarm process.


  var stopProcess = function stopProcess(process) {
    return new Promise(function (resolve, reject) {
      process.stderr.removeAllListeners('data');
      process.stdout.removeAllListeners('data');
      process.stdin.removeAllListeners('error');
      process.removeAllListeners('error');
      process.removeAllListeners('exit');
      process.kill('SIGINT');
      var killTimeout = setTimeout(function () {
        return process.kill('SIGKILL');
      }, 8000);
      process.once('close', function () {
        clearTimeout(killTimeout);
        resolve();
      });
    });
  }; // SwarmSetup -> (SwarmAPI -> Promise ()) -> Promise ()
  //   Receives a Swarm configuration object and a callback function. It then
  //   checks if a local Swarm node is running. If no local Swarm is found, it
  //   downloads the Swarm binaries to the dataDir (if not there), checksums,
  //   starts the Swarm process and calls the callback function with an API
  //   object using the local node. That callback must return a promise which
  //   will resolve when it is done using the API, so that this function can
  //   close the Swarm process properly. Returns a promise that resolves when the
  //   user is done with the API and the Swarm process is closed.
  //   TODO: check if Swarm process is already running (improve `isAvailable`)


  var local = function local(swarmSetup) {
    return function (useAPI) {
      return _isAvailable("http://localhost:8500").then(function (isAvailable) {
        return isAvailable ? useAPI(at("http://localhost:8500")).then(function () {}) : downloadBinary(swarmSetup.binPath, swarmSetup.archives).onData(function (data) {
          return (swarmSetup.onProgress || function () {})(data.length);
        }).then(function () {
          return startProcess(swarmSetup);
        }).then(function (process) {
          return useAPI(at("http://localhost:8500")).then(function () {
            return process;
          });
        }).then(stopProcess);
      });
    };
  }; // String ~> Promise Bool
  //   Returns true if Swarm is available on `url`.
  //   Perfoms a test upload to determine that.
  //   TODO: improve this?


  var _isAvailable = function isAvailable(swarmUrl) {
    var testFile = "test";
    var testHash = "c9a99c7d326dcc6316f32fe2625b311f6dc49a175e6877681ded93137d3569e7";
    return uploadData(swarmUrl)(testFile).then(function (hash) {
      return hash === testHash;
    })["catch"](function () {
      return false;
    });
  }; // String -> String ~> Promise Bool
  //   Returns a Promise which is true if that Swarm address is a directory.
  //   Determines that by checking that it (i) is a JSON, (ii) has a .entries.
  //   TODO: improve this?


  var isDirectory = function isDirectory(swarmUrl) {
    return function (hash) {
      return downloadData(swarmUrl)(hash).then(function (data) {
        try {
          return !!JSON.parse(toString(data)).entries;
        } catch (e) {
          return false;
        }
      });
    };
  }; // Uncurries a function; used to allow the f(x,y,z) style on exports.


  var uncurry = function uncurry(f) {
    return function (a, b, c, d, e) {
      var p; // Hardcoded because efficiency (`arguments` is very slow).

      if (typeof a !== "undefined") p = f(a);
      if (typeof b !== "undefined") p = f(b);
      if (typeof c !== "undefined") p = f(c);
      if (typeof d !== "undefined") p = f(d);
      if (typeof e !== "undefined") p = f(e);
      return p;
    };
  }; // () -> Promise Bool
  //   Not sure how to mock Swarm to test it properly. Ideas?


  var test = function test() {
    return Promise.resolve(true);
  }; // Uint8Array -> String


  var toString = function toString(uint8Array) {
    return bytes.toString(bytes.fromUint8Array(uint8Array));
  }; // String -> Uint8Array


  var fromString = function fromString(string) {
    return bytes.toUint8Array(bytes.fromString(string));
  }; // String -> SwarmAPI
  //   Fixes the `swarmUrl`, returning an API where you don't have to pass it.


  var at = function at(swarmUrl) {
    return {
      download: function download(hash, path) {
        return _download(swarmUrl)(hash)(path);
      },
      downloadData: uncurry(downloadData(swarmUrl)),
      downloadDataToDisk: uncurry(downloadDataToDisk(swarmUrl)),
      downloadDirectory: uncurry(downloadDirectory(swarmUrl)),
      downloadDirectoryToDisk: uncurry(downloadDirectoryToDisk(swarmUrl)),
      downloadEntries: uncurry(downloadEntries(swarmUrl)),
      downloadRoutes: uncurry(downloadRoutes(swarmUrl)),
      isAvailable: function isAvailable() {
        return _isAvailable(swarmUrl);
      },
      upload: function upload(arg) {
        return _upload(swarmUrl)(arg);
      },
      uploadData: uncurry(uploadData(swarmUrl)),
      uploadFile: uncurry(uploadFile(swarmUrl)),
      uploadFileFromDisk: uncurry(uploadFile(swarmUrl)),
      uploadDataFromDisk: uncurry(uploadDataFromDisk(swarmUrl)),
      uploadDirectory: uncurry(uploadDirectory(swarmUrl)),
      uploadDirectoryFromDisk: uncurry(uploadDirectoryFromDisk(swarmUrl)),
      uploadToManifest: uncurry(uploadToManifest(swarmUrl)),
      pick: pick,
      hash: hash,
      fromString: fromString,
      toString: toString
    };
  };

  return {
    at: at,
    local: local,
    download: _download,
    downloadBinary: downloadBinary,
    downloadData: downloadData,
    downloadDataToDisk: downloadDataToDisk,
    downloadDirectory: downloadDirectory,
    downloadDirectoryToDisk: downloadDirectoryToDisk,
    downloadEntries: downloadEntries,
    downloadRoutes: downloadRoutes,
    isAvailable: _isAvailable,
    startProcess: startProcess,
    stopProcess: stopProcess,
    upload: _upload,
    uploadData: uploadData,
    uploadDataFromDisk: uploadDataFromDisk,
    uploadFile: uploadFile,
    uploadFileFromDisk: uploadFileFromDisk,
    uploadDirectory: uploadDirectory,
    uploadDirectoryFromDisk: uploadDirectoryFromDisk,
    uploadToManifest: uploadToManifest,
    pick: pick,
    hash: hash,
    fromString: fromString,
    toString: toString
  };
};