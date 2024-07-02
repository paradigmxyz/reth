(function(){function r(e,n,t){function o(i,f){if(!n[i]){if(!e[i]){var c="function"==typeof require&&require;if(!f&&c)return c(i,!0);if(u)return u(i,!0);var a=new Error("Cannot find module '"+i+"'");throw a.code="MODULE_NOT_FOUND",a}var p=n[i]={exports:{}};e[i][0].call(p.exports,function(r){var n=e[i][1][r];return o(n||r)},p,p.exports,r,e,n,t)}return n[i].exports}for(var u="function"==typeof require&&require,i=0;i<t.length;i++)o(t[i]);return o}return r})()({1:[function(require,module,exports){
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
        for (var i = 0, l = a.length; i < a; ++i) {
          if (a[i] !== b[i]) return false;
        }
      }

      return true;
    };
  }; // String -> String -> String


  var rawUrl = function rawUrl(swarmUrl) {
    return function (hash) {
      return "".concat(swarmUrl, "/bzzr:/").concat(hash);
    };
  }; // String -> String -> Promise Uint8Array
  //   Gets the raw contents of a Swarm hash address.


  var downloadData = function downloadData(swarmUrl) {
    return function (hash) {
      return request(rawUrl(swarmUrl)(hash), {
        responseType: "arraybuffer"
      }).then(function (arrayBuffer) {
        var uint8Array = new Uint8Array(arrayBuffer);
        var error404 = [52, 48, 52, 32, 112, 97, 103, 101, 32, 110, 111, 116, 32, 102, 111, 117, 110, 100, 10];
        if (equals(uint8Array)(error404)) throw "Error 404.";
        return uint8Array;
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
      return request("".concat(swarmUrl, "/bzzr:/"), {
        body: typeof data === "string" ? fromString(data) : data,
        method: "POST"
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
            return request(url, opt).then(function (response) {
              if (response.indexOf("error") !== -1) {
                throw response;
              }

              return response;
            }).catch(function (e) {
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
        return startProcess(swarmSetup).then(resolve).catch(reject);
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
    }).catch(function () {
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
},{}]},{},[1])
//# sourceMappingURL=data:application/json;charset=utf-8;base64,eyJ2ZXJzaW9uIjozLCJzb3VyY2VzIjpbIm5vZGVfbW9kdWxlcy9icm93c2VyLXBhY2svX3ByZWx1ZGUuanMiLCJsaWIvc3dhcm0uanMiXSwibmFtZXMiOltdLCJtYXBwaW5ncyI6IkFBQUE7QUNBQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0E7QUFDQTtBQUNBO0FBQ0EiLCJmaWxlIjoiZ2VuZXJhdGVkLmpzIiwic291cmNlUm9vdCI6IiIsInNvdXJjZXNDb250ZW50IjpbIihmdW5jdGlvbigpe2Z1bmN0aW9uIHIoZSxuLHQpe2Z1bmN0aW9uIG8oaSxmKXtpZighbltpXSl7aWYoIWVbaV0pe3ZhciBjPVwiZnVuY3Rpb25cIj09dHlwZW9mIHJlcXVpcmUmJnJlcXVpcmU7aWYoIWYmJmMpcmV0dXJuIGMoaSwhMCk7aWYodSlyZXR1cm4gdShpLCEwKTt2YXIgYT1uZXcgRXJyb3IoXCJDYW5ub3QgZmluZCBtb2R1bGUgJ1wiK2krXCInXCIpO3Rocm93IGEuY29kZT1cIk1PRFVMRV9OT1RfRk9VTkRcIixhfXZhciBwPW5baV09e2V4cG9ydHM6e319O2VbaV1bMF0uY2FsbChwLmV4cG9ydHMsZnVuY3Rpb24ocil7dmFyIG49ZVtpXVsxXVtyXTtyZXR1cm4gbyhufHxyKX0scCxwLmV4cG9ydHMscixlLG4sdCl9cmV0dXJuIG5baV0uZXhwb3J0c31mb3IodmFyIHU9XCJmdW5jdGlvblwiPT10eXBlb2YgcmVxdWlyZSYmcmVxdWlyZSxpPTA7aTx0Lmxlbmd0aDtpKyspbyh0W2ldKTtyZXR1cm4gb31yZXR1cm4gcn0pKCkiLCIvLyBUT0RPOiB0aGlzIGlzIGEgdGVtcG9yYXJ5IGZpeCB0byBoaWRlIHRob3NlIGxpYnJhcmllcyBmcm9tIHRoZSBicm93c2VyLiBBXG4vLyBzbGlnaHRseSBiZXR0ZXIgbG9uZy10ZXJtIHNvbHV0aW9uIHdvdWxkIGJlIHRvIHNwbGl0IHRoaXMgZmlsZSBpbnRvIHR3byxcbi8vIHNlcGFyYXRpbmcgdGhlIGZ1bmN0aW9ucyB0aGF0IGFyZSB1c2VkIG9uIE5vZGUuanMgZnJvbSB0aGUgZnVuY3Rpb25zIHRoYXRcbi8vIGFyZSB1c2VkIG9ubHkgb24gdGhlIGJyb3dzZXIuXG5tb2R1bGUuZXhwb3J0cyA9IGZ1bmN0aW9uIChfcmVmKSB7XG4gIHZhciBmcyA9IF9yZWYuZnMsXG4gICAgICBmaWxlcyA9IF9yZWYuZmlsZXMsXG4gICAgICBvcyA9IF9yZWYub3MsXG4gICAgICBwYXRoID0gX3JlZi5wYXRoLFxuICAgICAgY2hpbGRfcHJvY2VzcyA9IF9yZWYuY2hpbGRfcHJvY2VzcyxcbiAgICAgIG1pbWV0eXBlID0gX3JlZi5taW1ldHlwZSxcbiAgICAgIGRlZmF1bHRBcmNoaXZlcyA9IF9yZWYuZGVmYXVsdEFyY2hpdmVzLFxuICAgICAgcmVxdWVzdCA9IF9yZWYucmVxdWVzdCxcbiAgICAgIGRvd25sb2FkVXJsID0gX3JlZi5kb3dubG9hZFVybCxcbiAgICAgIGJ5dGVzID0gX3JlZi5ieXRlcyxcbiAgICAgIGhhc2ggPSBfcmVmLmhhc2gsXG4gICAgICBwaWNrID0gX3JlZi5waWNrO1xuXG4gIC8vIOKIgCBhIC4gU3RyaW5nIC0+IEpTT04gLT4gTWFwIFN0cmluZyBhIC1vIE1hcCBTdHJpbmcgYVxuICAvLyAgIEluc2VydHMgYSBrZXkvdmFsIHBhaXIgaW4gYW4gb2JqZWN0IGltcHVyZWx5LlxuICB2YXIgaW1wdXJlSW5zZXJ0ID0gZnVuY3Rpb24gaW1wdXJlSW5zZXJ0KGtleSkge1xuICAgIHJldHVybiBmdW5jdGlvbiAodmFsKSB7XG4gICAgICByZXR1cm4gZnVuY3Rpb24gKG1hcCkge1xuICAgICAgICByZXR1cm4gbWFwW2tleV0gPSB2YWwsIG1hcDtcbiAgICAgIH07XG4gICAgfTtcbiAgfTsgLy8gU3RyaW5nIC0+IEpTT04gLT4gTWFwIFN0cmluZyBKU09OXG4gIC8vICAgTWVyZ2VzIGFuIGFycmF5IG9mIGtleXMgYW5kIGFuIGFycmF5IG9mIHZhbHMgaW50byBhbiBvYmplY3QuXG5cblxuICB2YXIgdG9NYXAgPSBmdW5jdGlvbiB0b01hcChrZXlzKSB7XG4gICAgcmV0dXJuIGZ1bmN0aW9uICh2YWxzKSB7XG4gICAgICB2YXIgbWFwID0ge307XG5cbiAgICAgIGZvciAodmFyIGkgPSAwLCBsID0ga2V5cy5sZW5ndGg7IGkgPCBsOyArK2kpIHtcbiAgICAgICAgbWFwW2tleXNbaV1dID0gdmFsc1tpXTtcbiAgICAgIH1cblxuICAgICAgcmV0dXJuIG1hcDtcbiAgICB9O1xuICB9OyAvLyDiiIAgYSAuIE1hcCBTdHJpbmcgYSAtPiBNYXAgU3RyaW5nIGEgLT4gTWFwIFN0cmluZyBhXG4gIC8vICAgTWVyZ2VzIHR3byBtYXBzIGludG8gb25lLlxuXG5cbiAgdmFyIG1lcmdlID0gZnVuY3Rpb24gbWVyZ2UoYSkge1xuICAgIHJldHVybiBmdW5jdGlvbiAoYikge1xuICAgICAgdmFyIG1hcCA9IHt9O1xuXG4gICAgICBmb3IgKHZhciBrZXkgaW4gYSkge1xuICAgICAgICBtYXBba2V5XSA9IGFba2V5XTtcbiAgICAgIH1cblxuICAgICAgZm9yICh2YXIgX2tleSBpbiBiKSB7XG4gICAgICAgIG1hcFtfa2V5XSA9IGJbX2tleV07XG4gICAgICB9XG5cbiAgICAgIHJldHVybiBtYXA7XG4gICAgfTtcbiAgfTsgLy8g4oiAIGEgLiBbYV0gLT4gW2FdIC0+IEJvb2xcblxuXG4gIHZhciBlcXVhbHMgPSBmdW5jdGlvbiBlcXVhbHMoYSkge1xuICAgIHJldHVybiBmdW5jdGlvbiAoYikge1xuICAgICAgaWYgKGEubGVuZ3RoICE9PSBiLmxlbmd0aCkge1xuICAgICAgICByZXR1cm4gZmFsc2U7XG4gICAgICB9IGVsc2Uge1xuICAgICAgICBmb3IgKHZhciBpID0gMCwgbCA9IGEubGVuZ3RoOyBpIDwgYTsgKytpKSB7XG4gICAgICAgICAgaWYgKGFbaV0gIT09IGJbaV0pIHJldHVybiBmYWxzZTtcbiAgICAgICAgfVxuICAgICAgfVxuXG4gICAgICByZXR1cm4gdHJ1ZTtcbiAgICB9O1xuICB9OyAvLyBTdHJpbmcgLT4gU3RyaW5nIC0+IFN0cmluZ1xuXG5cbiAgdmFyIHJhd1VybCA9IGZ1bmN0aW9uIHJhd1VybChzd2FybVVybCkge1xuICAgIHJldHVybiBmdW5jdGlvbiAoaGFzaCkge1xuICAgICAgcmV0dXJuIFwiXCIuY29uY2F0KHN3YXJtVXJsLCBcIi9ienpyOi9cIikuY29uY2F0KGhhc2gpO1xuICAgIH07XG4gIH07IC8vIFN0cmluZyAtPiBTdHJpbmcgLT4gUHJvbWlzZSBVaW50OEFycmF5XG4gIC8vICAgR2V0cyB0aGUgcmF3IGNvbnRlbnRzIG9mIGEgU3dhcm0gaGFzaCBhZGRyZXNzLlxuXG5cbiAgdmFyIGRvd25sb2FkRGF0YSA9IGZ1bmN0aW9uIGRvd25sb2FkRGF0YShzd2FybVVybCkge1xuICAgIHJldHVybiBmdW5jdGlvbiAoaGFzaCkge1xuICAgICAgcmV0dXJuIHJlcXVlc3QocmF3VXJsKHN3YXJtVXJsKShoYXNoKSwge1xuICAgICAgICByZXNwb25zZVR5cGU6IFwiYXJyYXlidWZmZXJcIlxuICAgICAgfSkudGhlbihmdW5jdGlvbiAoYXJyYXlCdWZmZXIpIHtcbiAgICAgICAgdmFyIHVpbnQ4QXJyYXkgPSBuZXcgVWludDhBcnJheShhcnJheUJ1ZmZlcik7XG4gICAgICAgIHZhciBlcnJvcjQwNCA9IFs1MiwgNDgsIDUyLCAzMiwgMTEyLCA5NywgMTAzLCAxMDEsIDMyLCAxMTAsIDExMSwgMTE2LCAzMiwgMTAyLCAxMTEsIDExNywgMTEwLCAxMDAsIDEwXTtcbiAgICAgICAgaWYgKGVxdWFscyh1aW50OEFycmF5KShlcnJvcjQwNCkpIHRocm93IFwiRXJyb3IgNDA0LlwiO1xuICAgICAgICByZXR1cm4gdWludDhBcnJheTtcbiAgICAgIH0pO1xuICAgIH07XG4gIH07IC8vIHR5cGUgRW50cnkgPSB7XCJ0eXBlXCI6IFN0cmluZywgXCJoYXNoXCI6IFN0cmluZ31cbiAgLy8gdHlwZSBGaWxlID0ge1widHlwZVwiOiBTdHJpbmcsIFwiZGF0YVwiOiBVaW50OEFycmF5fVxuICAvLyBTdHJpbmcgLT4gU3RyaW5nIC0+IFByb21pc2UgKE1hcCBTdHJpbmcgRW50cnkpXG4gIC8vICAgU29sdmVzIHRoZSBtYW5pZmVzdCBvZiBhIFN3YXJtIGFkZHJlc3MgcmVjdXJzaXZlbHkuXG4gIC8vICAgUmV0dXJucyBhIG1hcCBmcm9tIGZ1bGwgcGF0aHMgdG8gZW50cmllcy5cblxuXG4gIHZhciBkb3dubG9hZEVudHJpZXMgPSBmdW5jdGlvbiBkb3dubG9hZEVudHJpZXMoc3dhcm1VcmwpIHtcbiAgICByZXR1cm4gZnVuY3Rpb24gKGhhc2gpIHtcbiAgICAgIHZhciBzZWFyY2ggPSBmdW5jdGlvbiBzZWFyY2goaGFzaCkge1xuICAgICAgICByZXR1cm4gZnVuY3Rpb24gKHBhdGgpIHtcbiAgICAgICAgICByZXR1cm4gZnVuY3Rpb24gKHJvdXRlcykge1xuICAgICAgICAgICAgLy8gRm9ybWF0cyBhbiBlbnRyeSB0byB0aGUgU3dhcm0uanMgdHlwZS5cbiAgICAgICAgICAgIHZhciBmb3JtYXQgPSBmdW5jdGlvbiBmb3JtYXQoZW50cnkpIHtcbiAgICAgICAgICAgICAgcmV0dXJuIHtcbiAgICAgICAgICAgICAgICB0eXBlOiBlbnRyeS5jb250ZW50VHlwZSxcbiAgICAgICAgICAgICAgICBoYXNoOiBlbnRyeS5oYXNoXG4gICAgICAgICAgICAgIH07XG4gICAgICAgICAgICB9OyAvLyBUbyBkb3dubG9hZCBhIHNpbmdsZSBlbnRyeTpcbiAgICAgICAgICAgIC8vICAgaWYgdHlwZSBpcyBienotbWFuaWZlc3QsIGdvIGRlZXBlclxuICAgICAgICAgICAgLy8gICBpZiBub3QsIGFkZCBpdCB0byB0aGUgcm91dGluZyB0YWJsZVxuXG5cbiAgICAgICAgICAgIHZhciBkb3dubG9hZEVudHJ5ID0gZnVuY3Rpb24gZG93bmxvYWRFbnRyeShlbnRyeSkge1xuICAgICAgICAgICAgICBpZiAoZW50cnkucGF0aCA9PT0gdW5kZWZpbmVkKSB7XG4gICAgICAgICAgICAgICAgcmV0dXJuIFByb21pc2UucmVzb2x2ZSgpO1xuICAgICAgICAgICAgICB9IGVsc2Uge1xuICAgICAgICAgICAgICAgIHJldHVybiBlbnRyeS5jb250ZW50VHlwZSA9PT0gXCJhcHBsaWNhdGlvbi9ienotbWFuaWZlc3QranNvblwiID8gc2VhcmNoKGVudHJ5Lmhhc2gpKHBhdGggKyBlbnRyeS5wYXRoKShyb3V0ZXMpIDogUHJvbWlzZS5yZXNvbHZlKGltcHVyZUluc2VydChwYXRoICsgZW50cnkucGF0aCkoZm9ybWF0KGVudHJ5KSkocm91dGVzKSk7XG4gICAgICAgICAgICAgIH1cbiAgICAgICAgICAgIH07IC8vIERvd25sb2FkcyB0aGUgaW5pdGlhbCBtYW5pZmVzdCBhbmQgdGhlbiBlYWNoIGVudHJ5LlxuXG5cbiAgICAgICAgICAgIHJldHVybiBkb3dubG9hZERhdGEoc3dhcm1VcmwpKGhhc2gpLnRoZW4oZnVuY3Rpb24gKHRleHQpIHtcbiAgICAgICAgICAgICAgcmV0dXJuIEpTT04ucGFyc2UodG9TdHJpbmcodGV4dCkpLmVudHJpZXM7XG4gICAgICAgICAgICB9KS50aGVuKGZ1bmN0aW9uIChlbnRyaWVzKSB7XG4gICAgICAgICAgICAgIHJldHVybiBQcm9taXNlLmFsbChlbnRyaWVzLm1hcChkb3dubG9hZEVudHJ5KSk7XG4gICAgICAgICAgICB9KS50aGVuKGZ1bmN0aW9uICgpIHtcbiAgICAgICAgICAgICAgcmV0dXJuIHJvdXRlcztcbiAgICAgICAgICAgIH0pO1xuICAgICAgICAgIH07XG4gICAgICAgIH07XG4gICAgICB9O1xuXG4gICAgICByZXR1cm4gc2VhcmNoKGhhc2gpKFwiXCIpKHt9KTtcbiAgICB9O1xuICB9OyAvLyBTdHJpbmcgLT4gU3RyaW5nIC0+IFByb21pc2UgKE1hcCBTdHJpbmcgU3RyaW5nKVxuICAvLyAgIFNhbWUgYXMgYGRvd25sb2FkRW50cmllc2AsIGJ1dCByZXR1cm5zIG9ubHkgaGFzaGVzIChubyB0eXBlcykuXG5cblxuICB2YXIgZG93bmxvYWRSb3V0ZXMgPSBmdW5jdGlvbiBkb3dubG9hZFJvdXRlcyhzd2FybVVybCkge1xuICAgIHJldHVybiBmdW5jdGlvbiAoaGFzaCkge1xuICAgICAgcmV0dXJuIGRvd25sb2FkRW50cmllcyhzd2FybVVybCkoaGFzaCkudGhlbihmdW5jdGlvbiAoZW50cmllcykge1xuICAgICAgICByZXR1cm4gdG9NYXAoT2JqZWN0LmtleXMoZW50cmllcykpKE9iamVjdC5rZXlzKGVudHJpZXMpLm1hcChmdW5jdGlvbiAocm91dGUpIHtcbiAgICAgICAgICByZXR1cm4gZW50cmllc1tyb3V0ZV0uaGFzaDtcbiAgICAgICAgfSkpO1xuICAgICAgfSk7XG4gICAgfTtcbiAgfTsgLy8gU3RyaW5nIC0+IFN0cmluZyAtPiBQcm9taXNlIChNYXAgU3RyaW5nIEZpbGUpXG4gIC8vICAgR2V0cyB0aGUgZW50aXJlIGRpcmVjdG9yeSB0cmVlIGluIGEgU3dhcm0gYWRkcmVzcy5cbiAgLy8gICBSZXR1cm5zIGEgcHJvbWlzZSBtYXBwaW5nIHBhdGhzIHRvIGZpbGUgY29udGVudHMuXG5cblxuICB2YXIgZG93bmxvYWREaXJlY3RvcnkgPSBmdW5jdGlvbiBkb3dubG9hZERpcmVjdG9yeShzd2FybVVybCkge1xuICAgIHJldHVybiBmdW5jdGlvbiAoaGFzaCkge1xuICAgICAgcmV0dXJuIGRvd25sb2FkRW50cmllcyhzd2FybVVybCkoaGFzaCkudGhlbihmdW5jdGlvbiAoZW50cmllcykge1xuICAgICAgICB2YXIgcGF0aHMgPSBPYmplY3Qua2V5cyhlbnRyaWVzKTtcbiAgICAgICAgdmFyIGhhc2hzID0gcGF0aHMubWFwKGZ1bmN0aW9uIChwYXRoKSB7XG4gICAgICAgICAgcmV0dXJuIGVudHJpZXNbcGF0aF0uaGFzaDtcbiAgICAgICAgfSk7XG4gICAgICAgIHZhciB0eXBlcyA9IHBhdGhzLm1hcChmdW5jdGlvbiAocGF0aCkge1xuICAgICAgICAgIHJldHVybiBlbnRyaWVzW3BhdGhdLnR5cGU7XG4gICAgICAgIH0pO1xuICAgICAgICB2YXIgZGF0YXMgPSBoYXNocy5tYXAoZG93bmxvYWREYXRhKHN3YXJtVXJsKSk7XG5cbiAgICAgICAgdmFyIGZpbGVzID0gZnVuY3Rpb24gZmlsZXMoZGF0YXMpIHtcbiAgICAgICAgICByZXR1cm4gZGF0YXMubWFwKGZ1bmN0aW9uIChkYXRhLCBpKSB7XG4gICAgICAgICAgICByZXR1cm4ge1xuICAgICAgICAgICAgICB0eXBlOiB0eXBlc1tpXSxcbiAgICAgICAgICAgICAgZGF0YTogZGF0YVxuICAgICAgICAgICAgfTtcbiAgICAgICAgICB9KTtcbiAgICAgICAgfTtcblxuICAgICAgICByZXR1cm4gUHJvbWlzZS5hbGwoZGF0YXMpLnRoZW4oZnVuY3Rpb24gKGRhdGFzKSB7XG4gICAgICAgICAgcmV0dXJuIHRvTWFwKHBhdGhzKShmaWxlcyhkYXRhcykpO1xuICAgICAgICB9KTtcbiAgICAgIH0pO1xuICAgIH07XG4gIH07IC8vIFN0cmluZyAtPiBTdHJpbmcgLT4gU3RyaW5nIC0+IFByb21pc2UgU3RyaW5nXG4gIC8vICAgR2V0cyB0aGUgcmF3IGNvbnRlbnRzIG9mIGEgU3dhcm0gaGFzaCBhZGRyZXNzLlxuICAvLyAgIFJldHVybnMgYSBwcm9taXNlIHdpdGggdGhlIGRvd25sb2FkZWQgZmlsZSBwYXRoLlxuXG5cbiAgdmFyIGRvd25sb2FkRGF0YVRvRGlzayA9IGZ1bmN0aW9uIGRvd25sb2FkRGF0YVRvRGlzayhzd2FybVVybCkge1xuICAgIHJldHVybiBmdW5jdGlvbiAoaGFzaCkge1xuICAgICAgcmV0dXJuIGZ1bmN0aW9uIChmaWxlUGF0aCkge1xuICAgICAgICByZXR1cm4gZmlsZXMuZG93bmxvYWQocmF3VXJsKHN3YXJtVXJsKShoYXNoKSkoZmlsZVBhdGgpO1xuICAgICAgfTtcbiAgICB9O1xuICB9OyAvLyBTdHJpbmcgLT4gU3RyaW5nIC0+IFN0cmluZyAtPiBQcm9taXNlIChNYXAgU3RyaW5nIFN0cmluZylcbiAgLy8gICBHZXRzIHRoZSBlbnRpcmUgZGlyZWN0b3J5IHRyZWUgaW4gYSBTd2FybSBhZGRyZXNzLlxuICAvLyAgIFJldHVybnMgYSBwcm9taXNlIG1hcHBpbmcgcGF0aHMgdG8gZmlsZSBjb250ZW50cy5cblxuXG4gIHZhciBkb3dubG9hZERpcmVjdG9yeVRvRGlzayA9IGZ1bmN0aW9uIGRvd25sb2FkRGlyZWN0b3J5VG9EaXNrKHN3YXJtVXJsKSB7XG4gICAgcmV0dXJuIGZ1bmN0aW9uIChoYXNoKSB7XG4gICAgICByZXR1cm4gZnVuY3Rpb24gKGRpclBhdGgpIHtcbiAgICAgICAgcmV0dXJuIGRvd25sb2FkUm91dGVzKHN3YXJtVXJsKShoYXNoKS50aGVuKGZ1bmN0aW9uIChyb3V0aW5nVGFibGUpIHtcbiAgICAgICAgICB2YXIgZG93bmxvYWRzID0gW107XG5cbiAgICAgICAgICBmb3IgKHZhciByb3V0ZSBpbiByb3V0aW5nVGFibGUpIHtcbiAgICAgICAgICAgIGlmIChyb3V0ZS5sZW5ndGggPiAwKSB7XG4gICAgICAgICAgICAgIHZhciBmaWxlUGF0aCA9IHBhdGguam9pbihkaXJQYXRoLCByb3V0ZSk7XG4gICAgICAgICAgICAgIGRvd25sb2Fkcy5wdXNoKGRvd25sb2FkRGF0YVRvRGlzayhzd2FybVVybCkocm91dGluZ1RhYmxlW3JvdXRlXSkoZmlsZVBhdGgpKTtcbiAgICAgICAgICAgIH1cblxuICAgICAgICAgICAgO1xuICAgICAgICAgIH1cblxuICAgICAgICAgIDtcbiAgICAgICAgICByZXR1cm4gUHJvbWlzZS5hbGwoZG93bmxvYWRzKS50aGVuKGZ1bmN0aW9uICgpIHtcbiAgICAgICAgICAgIHJldHVybiBkaXJQYXRoO1xuICAgICAgICAgIH0pO1xuICAgICAgICB9KTtcbiAgICAgIH07XG4gICAgfTtcbiAgfTsgLy8gU3RyaW5nIC0+IFVpbnQ4QXJyYXkgLT4gUHJvbWlzZSBTdHJpbmdcbiAgLy8gICBVcGxvYWRzIHJhdyBkYXRhIHRvIFN3YXJtLlxuICAvLyAgIFJldHVybnMgYSBwcm9taXNlIHdpdGggdGhlIHVwbG9hZGVkIGhhc2guXG5cblxuICB2YXIgdXBsb2FkRGF0YSA9IGZ1bmN0aW9uIHVwbG9hZERhdGEoc3dhcm1VcmwpIHtcbiAgICByZXR1cm4gZnVuY3Rpb24gKGRhdGEpIHtcbiAgICAgIHJldHVybiByZXF1ZXN0KFwiXCIuY29uY2F0KHN3YXJtVXJsLCBcIi9ienpyOi9cIiksIHtcbiAgICAgICAgYm9keTogdHlwZW9mIGRhdGEgPT09IFwic3RyaW5nXCIgPyBmcm9tU3RyaW5nKGRhdGEpIDogZGF0YSxcbiAgICAgICAgbWV0aG9kOiBcIlBPU1RcIlxuICAgICAgfSk7XG4gICAgfTtcbiAgfTsgLy8gU3RyaW5nIC0+IFN0cmluZyAtPiBTdHJpbmcgLT4gRmlsZSAtPiBQcm9taXNlIFN0cmluZ1xuICAvLyAgIFVwbG9hZHMgYSBmaWxlIHRvIHRoZSBTd2FybSBtYW5pZmVzdCBhdCBhIGdpdmVuIGhhc2gsIHVuZGVyIGEgc3BlY2lmaWNcbiAgLy8gICByb3V0ZS4gUmV0dXJucyBhIHByb21pc2UgY29udGFpbmluZyB0aGUgdXBsb2FkZWQgaGFzaC5cbiAgLy8gICBGSVhNRTogZm9yIHNvbWUgcmVhc29ucyBTd2FybS1HYXRld2F5cyBpcyBzb21ldGltZXMgcmV0dXJuaW5nXG4gIC8vICAgZXJyb3IgNDA0wqAoYmFkIHJlcXVlc3QpLCBzbyB3ZSByZXRyeSB1cCB0byAzIHRpbWVzLiBXaHk/XG5cblxuICB2YXIgdXBsb2FkVG9NYW5pZmVzdCA9IGZ1bmN0aW9uIHVwbG9hZFRvTWFuaWZlc3Qoc3dhcm1VcmwpIHtcbiAgICByZXR1cm4gZnVuY3Rpb24gKGhhc2gpIHtcbiAgICAgIHJldHVybiBmdW5jdGlvbiAocm91dGUpIHtcbiAgICAgICAgcmV0dXJuIGZ1bmN0aW9uIChmaWxlKSB7XG4gICAgICAgICAgdmFyIGF0dGVtcHQgPSBmdW5jdGlvbiBhdHRlbXB0KG4pIHtcbiAgICAgICAgICAgIHZhciBzbGFzaFJvdXRlID0gcm91dGVbMF0gPT09IFwiL1wiID8gcm91dGUgOiBcIi9cIiArIHJvdXRlO1xuICAgICAgICAgICAgdmFyIHVybCA9IFwiXCIuY29uY2F0KHN3YXJtVXJsLCBcIi9ieno6L1wiKS5jb25jYXQoaGFzaCkuY29uY2F0KHNsYXNoUm91dGUpO1xuICAgICAgICAgICAgdmFyIG9wdCA9IHtcbiAgICAgICAgICAgICAgbWV0aG9kOiBcIlBVVFwiLFxuICAgICAgICAgICAgICBoZWFkZXJzOiB7XG4gICAgICAgICAgICAgICAgXCJDb250ZW50LVR5cGVcIjogZmlsZS50eXBlXG4gICAgICAgICAgICAgIH0sXG4gICAgICAgICAgICAgIGJvZHk6IGZpbGUuZGF0YVxuICAgICAgICAgICAgfTtcbiAgICAgICAgICAgIHJldHVybiByZXF1ZXN0KHVybCwgb3B0KS50aGVuKGZ1bmN0aW9uIChyZXNwb25zZSkge1xuICAgICAgICAgICAgICBpZiAocmVzcG9uc2UuaW5kZXhPZihcImVycm9yXCIpICE9PSAtMSkge1xuICAgICAgICAgICAgICAgIHRocm93IHJlc3BvbnNlO1xuICAgICAgICAgICAgICB9XG5cbiAgICAgICAgICAgICAgcmV0dXJuIHJlc3BvbnNlO1xuICAgICAgICAgICAgfSkuY2F0Y2goZnVuY3Rpb24gKGUpIHtcbiAgICAgICAgICAgICAgcmV0dXJuIG4gPiAwICYmIGF0dGVtcHQobiAtIDEpO1xuICAgICAgICAgICAgfSk7XG4gICAgICAgICAgfTtcblxuICAgICAgICAgIHJldHVybiBhdHRlbXB0KDMpO1xuICAgICAgICB9O1xuICAgICAgfTtcbiAgICB9O1xuICB9OyAvLyBTdHJpbmcgLT4ge3R5cGU6IFN0cmluZywgZGF0YTogVWludDhBcnJheX0gLT4gUHJvbWlzZSBTdHJpbmdcblxuXG4gIHZhciB1cGxvYWRGaWxlID0gZnVuY3Rpb24gdXBsb2FkRmlsZShzd2FybVVybCkge1xuICAgIHJldHVybiBmdW5jdGlvbiAoZmlsZSkge1xuICAgICAgcmV0dXJuIHVwbG9hZERpcmVjdG9yeShzd2FybVVybCkoe1xuICAgICAgICBcIlwiOiBmaWxlXG4gICAgICB9KTtcbiAgICB9O1xuICB9OyAvLyBTdHJpbmcgLT4gU3RyaW5nIC0+IFByb21pc2UgU3RyaW5nXG5cblxuICB2YXIgdXBsb2FkRmlsZUZyb21EaXNrID0gZnVuY3Rpb24gdXBsb2FkRmlsZUZyb21EaXNrKHN3YXJtVXJsKSB7XG4gICAgcmV0dXJuIGZ1bmN0aW9uIChmaWxlUGF0aCkge1xuICAgICAgcmV0dXJuIGZzLnJlYWRGaWxlKGZpbGVQYXRoKS50aGVuKGZ1bmN0aW9uIChkYXRhKSB7XG4gICAgICAgIHJldHVybiB1cGxvYWRGaWxlKHN3YXJtVXJsKSh7XG4gICAgICAgICAgdHlwZTogbWltZXR5cGUubG9va3VwKGZpbGVQYXRoKSxcbiAgICAgICAgICBkYXRhOiBkYXRhXG4gICAgICAgIH0pO1xuICAgICAgfSk7XG4gICAgfTtcbiAgfTsgLy8gU3RyaW5nIC0+IE1hcCBTdHJpbmcgRmlsZSAtPiBQcm9taXNlIFN0cmluZ1xuICAvLyAgIFVwbG9hZHMgYSBkaXJlY3RvcnkgdG8gU3dhcm0uIFRoZSBkaXJlY3RvcnkgaXNcbiAgLy8gICByZXByZXNlbnRlZCBhcyBhIG1hcCBvZiByb3V0ZXMgYW5kIGZpbGVzLlxuICAvLyAgIEEgZGVmYXVsdCBwYXRoIGlzIGVuY29kZWQgYnkgaGF2aW5nIGEgXCJcIiByb3V0ZS5cblxuXG4gIHZhciB1cGxvYWREaXJlY3RvcnkgPSBmdW5jdGlvbiB1cGxvYWREaXJlY3Rvcnkoc3dhcm1VcmwpIHtcbiAgICByZXR1cm4gZnVuY3Rpb24gKGRpcmVjdG9yeSkge1xuICAgICAgcmV0dXJuIHVwbG9hZERhdGEoc3dhcm1VcmwpKFwie31cIikudGhlbihmdW5jdGlvbiAoaGFzaCkge1xuICAgICAgICB2YXIgdXBsb2FkUm91dGUgPSBmdW5jdGlvbiB1cGxvYWRSb3V0ZShyb3V0ZSkge1xuICAgICAgICAgIHJldHVybiBmdW5jdGlvbiAoaGFzaCkge1xuICAgICAgICAgICAgcmV0dXJuIHVwbG9hZFRvTWFuaWZlc3Qoc3dhcm1VcmwpKGhhc2gpKHJvdXRlKShkaXJlY3Rvcnlbcm91dGVdKTtcbiAgICAgICAgICB9O1xuICAgICAgICB9O1xuXG4gICAgICAgIHZhciB1cGxvYWRUb0hhc2ggPSBmdW5jdGlvbiB1cGxvYWRUb0hhc2goaGFzaCwgcm91dGUpIHtcbiAgICAgICAgICByZXR1cm4gaGFzaC50aGVuKHVwbG9hZFJvdXRlKHJvdXRlKSk7XG4gICAgICAgIH07XG5cbiAgICAgICAgcmV0dXJuIE9iamVjdC5rZXlzKGRpcmVjdG9yeSkucmVkdWNlKHVwbG9hZFRvSGFzaCwgUHJvbWlzZS5yZXNvbHZlKGhhc2gpKTtcbiAgICAgIH0pO1xuICAgIH07XG4gIH07IC8vIFN0cmluZyAtPiBQcm9taXNlIFN0cmluZ1xuXG5cbiAgdmFyIHVwbG9hZERhdGFGcm9tRGlzayA9IGZ1bmN0aW9uIHVwbG9hZERhdGFGcm9tRGlzayhzd2FybVVybCkge1xuICAgIHJldHVybiBmdW5jdGlvbiAoZmlsZVBhdGgpIHtcbiAgICAgIHJldHVybiBmcy5yZWFkRmlsZShmaWxlUGF0aCkudGhlbih1cGxvYWREYXRhKHN3YXJtVXJsKSk7XG4gICAgfTtcbiAgfTsgLy8gU3RyaW5nIC0+IE51bGxhYmxlIFN0cmluZyAtPiBTdHJpbmcgLT4gUHJvbWlzZSBTdHJpbmdcblxuXG4gIHZhciB1cGxvYWREaXJlY3RvcnlGcm9tRGlzayA9IGZ1bmN0aW9uIHVwbG9hZERpcmVjdG9yeUZyb21EaXNrKHN3YXJtVXJsKSB7XG4gICAgcmV0dXJuIGZ1bmN0aW9uIChkZWZhdWx0UGF0aCkge1xuICAgICAgcmV0dXJuIGZ1bmN0aW9uIChkaXJQYXRoKSB7XG4gICAgICAgIHJldHVybiBmaWxlcy5kaXJlY3RvcnlUcmVlKGRpclBhdGgpLnRoZW4oZnVuY3Rpb24gKGZ1bGxQYXRocykge1xuICAgICAgICAgIHJldHVybiBQcm9taXNlLmFsbChmdWxsUGF0aHMubWFwKGZ1bmN0aW9uIChwYXRoKSB7XG4gICAgICAgICAgICByZXR1cm4gZnMucmVhZEZpbGUocGF0aCk7XG4gICAgICAgICAgfSkpLnRoZW4oZnVuY3Rpb24gKGRhdGFzKSB7XG4gICAgICAgICAgICB2YXIgcGF0aHMgPSBmdWxsUGF0aHMubWFwKGZ1bmN0aW9uIChwYXRoKSB7XG4gICAgICAgICAgICAgIHJldHVybiBwYXRoLnNsaWNlKGRpclBhdGgubGVuZ3RoKTtcbiAgICAgICAgICAgIH0pO1xuICAgICAgICAgICAgdmFyIHR5cGVzID0gZnVsbFBhdGhzLm1hcChmdW5jdGlvbiAocGF0aCkge1xuICAgICAgICAgICAgICByZXR1cm4gbWltZXR5cGUubG9va3VwKHBhdGgpIHx8IFwidGV4dC9wbGFpblwiO1xuICAgICAgICAgICAgfSk7XG4gICAgICAgICAgICByZXR1cm4gdG9NYXAocGF0aHMpKGRhdGFzLm1hcChmdW5jdGlvbiAoZGF0YSwgaSkge1xuICAgICAgICAgICAgICByZXR1cm4ge1xuICAgICAgICAgICAgICAgIHR5cGU6IHR5cGVzW2ldLFxuICAgICAgICAgICAgICAgIGRhdGE6IGRhdGFcbiAgICAgICAgICAgICAgfTtcbiAgICAgICAgICAgIH0pKTtcbiAgICAgICAgICB9KTtcbiAgICAgICAgfSkudGhlbihmdW5jdGlvbiAoZGlyZWN0b3J5KSB7XG4gICAgICAgICAgcmV0dXJuIG1lcmdlKGRlZmF1bHRQYXRoID8ge1xuICAgICAgICAgICAgXCJcIjogZGlyZWN0b3J5W2RlZmF1bHRQYXRoXVxuICAgICAgICAgIH0gOiB7fSkoZGlyZWN0b3J5KTtcbiAgICAgICAgfSkudGhlbih1cGxvYWREaXJlY3Rvcnkoc3dhcm1VcmwpKTtcbiAgICAgIH07XG4gICAgfTtcbiAgfTsgLy8gU3RyaW5nIC0+IFVwbG9hZEluZm8gLT4gUHJvbWlzZSBTdHJpbmdcbiAgLy8gICBTaW1wbGlmaWVkIG11bHRpLXR5cGUgdXBsb2FkIHdoaWNoIGNhbGxzIHRoZSBjb3JyZWN0XG4gIC8vICAgb25lIGJhc2VkIG9uIHRoZSB0eXBlIG9mIHRoZSBhcmd1bWVudCBnaXZlbi5cblxuXG4gIHZhciBfdXBsb2FkID0gZnVuY3Rpb24gdXBsb2FkKHN3YXJtVXJsKSB7XG4gICAgcmV0dXJuIGZ1bmN0aW9uIChhcmcpIHtcbiAgICAgIC8vIFVwbG9hZCByYXcgZGF0YSBmcm9tIGJyb3dzZXJcbiAgICAgIGlmIChhcmcucGljayA9PT0gXCJkYXRhXCIpIHtcbiAgICAgICAgcmV0dXJuIHBpY2suZGF0YSgpLnRoZW4odXBsb2FkRGF0YShzd2FybVVybCkpOyAvLyBVcGxvYWQgYSBmaWxlIGZyb20gYnJvd3NlclxuICAgICAgfSBlbHNlIGlmIChhcmcucGljayA9PT0gXCJmaWxlXCIpIHtcbiAgICAgICAgcmV0dXJuIHBpY2suZmlsZSgpLnRoZW4odXBsb2FkRmlsZShzd2FybVVybCkpOyAvLyBVcGxvYWQgYSBkaXJlY3RvcnkgZnJvbSBicm93c2VyXG4gICAgICB9IGVsc2UgaWYgKGFyZy5waWNrID09PSBcImRpcmVjdG9yeVwiKSB7XG4gICAgICAgIHJldHVybiBwaWNrLmRpcmVjdG9yeSgpLnRoZW4odXBsb2FkRGlyZWN0b3J5KHN3YXJtVXJsKSk7IC8vIFVwbG9hZCBkaXJlY3RvcnkvZmlsZSBmcm9tIGRpc2tcbiAgICAgIH0gZWxzZSBpZiAoYXJnLnBhdGgpIHtcbiAgICAgICAgc3dpdGNoIChhcmcua2luZCkge1xuICAgICAgICAgIGNhc2UgXCJkYXRhXCI6XG4gICAgICAgICAgICByZXR1cm4gdXBsb2FkRGF0YUZyb21EaXNrKHN3YXJtVXJsKShhcmcucGF0aCk7XG5cbiAgICAgICAgICBjYXNlIFwiZmlsZVwiOlxuICAgICAgICAgICAgcmV0dXJuIHVwbG9hZEZpbGVGcm9tRGlzayhzd2FybVVybCkoYXJnLnBhdGgpO1xuXG4gICAgICAgICAgY2FzZSBcImRpcmVjdG9yeVwiOlxuICAgICAgICAgICAgcmV0dXJuIHVwbG9hZERpcmVjdG9yeUZyb21EaXNrKHN3YXJtVXJsKShhcmcuZGVmYXVsdEZpbGUpKGFyZy5wYXRoKTtcbiAgICAgICAgfVxuXG4gICAgICAgIDsgLy8gVXBsb2FkIFVURi04IHN0cmluZyBvciByYXcgZGF0YSAoYnVmZmVyKVxuICAgICAgfSBlbHNlIGlmIChhcmcubGVuZ3RoIHx8IHR5cGVvZiBhcmcgPT09IFwic3RyaW5nXCIpIHtcbiAgICAgICAgcmV0dXJuIHVwbG9hZERhdGEoc3dhcm1VcmwpKGFyZyk7IC8vIFVwbG9hZCBkaXJlY3Rvcnkgd2l0aCBKU09OXG4gICAgICB9IGVsc2UgaWYgKGFyZyBpbnN0YW5jZW9mIE9iamVjdCkge1xuICAgICAgICByZXR1cm4gdXBsb2FkRGlyZWN0b3J5KHN3YXJtVXJsKShhcmcpO1xuICAgICAgfVxuXG4gICAgICByZXR1cm4gUHJvbWlzZS5yZWplY3QobmV3IEVycm9yKFwiQmFkIGFyZ3VtZW50c1wiKSk7XG4gICAgfTtcbiAgfTsgLy8gU3RyaW5nIC0+IFN0cmluZyAtPiBOdWxsYWJsZSBTdHJpbmcgLT4gUHJvbWlzZSAoU3RyaW5nIHwgVWludDhBcnJheSB8IE1hcCBTdHJpbmcgVWludDhBcnJheSlcbiAgLy8gICBTaW1wbGlmaWVkIG11bHRpLXR5cGUgZG93bmxvYWQgd2hpY2ggY2FsbHMgdGhlIGNvcnJlY3QgZnVuY3Rpb24gYmFzZWQgb25cbiAgLy8gICB0aGUgdHlwZSBvZiB0aGUgYXJndW1lbnQgZ2l2ZW4sIGFuZCBvbiB3aGV0aGVyIHRoZSBTd3dhcm0gYWRkcmVzcyBoYXMgYVxuICAvLyAgIGRpcmVjdG9yeSBvciBhIGZpbGUuXG5cblxuICB2YXIgX2Rvd25sb2FkID0gZnVuY3Rpb24gZG93bmxvYWQoc3dhcm1VcmwpIHtcbiAgICByZXR1cm4gZnVuY3Rpb24gKGhhc2gpIHtcbiAgICAgIHJldHVybiBmdW5jdGlvbiAocGF0aCkge1xuICAgICAgICByZXR1cm4gaXNEaXJlY3Rvcnkoc3dhcm1VcmwpKGhhc2gpLnRoZW4oZnVuY3Rpb24gKGlzRGlyKSB7XG4gICAgICAgICAgaWYgKGlzRGlyKSB7XG4gICAgICAgICAgICByZXR1cm4gcGF0aCA/IGRvd25sb2FkRGlyZWN0b3J5VG9EaXNrKHN3YXJtVXJsKShoYXNoKShwYXRoKSA6IGRvd25sb2FkRGlyZWN0b3J5KHN3YXJtVXJsKShoYXNoKTtcbiAgICAgICAgICB9IGVsc2Uge1xuICAgICAgICAgICAgcmV0dXJuIHBhdGggPyBkb3dubG9hZERhdGFUb0Rpc2soc3dhcm1VcmwpKGhhc2gpKHBhdGgpIDogZG93bmxvYWREYXRhKHN3YXJtVXJsKShoYXNoKTtcbiAgICAgICAgICB9XG4gICAgICAgIH0pO1xuICAgICAgfTtcbiAgICB9O1xuICB9OyAvLyBTdHJpbmcgLT4gUHJvbWlzZSBTdHJpbmdcbiAgLy8gICBEb3dubG9hZHMgdGhlIFN3YXJtIGJpbmFyaWVzIGludG8gYSBwYXRoLiBSZXR1cm5zIGEgcHJvbWlzZSB0aGF0IG9ubHlcbiAgLy8gICByZXNvbHZlcyB3aGVuIHRoZSBleGFjdCBTd2FybSBmaWxlIGlzIHRoZXJlLCBhbmQgdmVyaWZpZWQgdG8gYmUgY29ycmVjdC5cbiAgLy8gICBJZiBpdCB3YXMgYWxyZWFkeSB0aGVyZSB0byBiZWdpbiB3aXRoLCBza2lwcyB0aGUgZG93bmxvYWQuXG5cblxuICB2YXIgZG93bmxvYWRCaW5hcnkgPSBmdW5jdGlvbiBkb3dubG9hZEJpbmFyeShwYXRoLCBhcmNoaXZlcykge1xuICAgIHZhciBzeXN0ZW0gPSBvcy5wbGF0Zm9ybSgpLnJlcGxhY2UoXCJ3aW4zMlwiLCBcIndpbmRvd3NcIikgKyBcIi1cIiArIChvcy5hcmNoKCkgPT09IFwieDY0XCIgPyBcImFtZDY0XCIgOiBcIjM4NlwiKTtcbiAgICB2YXIgYXJjaGl2ZSA9IChhcmNoaXZlcyB8fCBkZWZhdWx0QXJjaGl2ZXMpW3N5c3RlbV07XG4gICAgdmFyIGFyY2hpdmVVcmwgPSBkb3dubG9hZFVybCArIGFyY2hpdmUuYXJjaGl2ZSArIFwiLnRhci5nelwiO1xuICAgIHZhciBhcmNoaXZlTUQ1ID0gYXJjaGl2ZS5hcmNoaXZlTUQ1O1xuICAgIHZhciBiaW5hcnlNRDUgPSBhcmNoaXZlLmJpbmFyeU1ENTtcbiAgICByZXR1cm4gZmlsZXMuc2FmZURvd25sb2FkQXJjaGl2ZWQoYXJjaGl2ZVVybCkoYXJjaGl2ZU1ENSkoYmluYXJ5TUQ1KShwYXRoKTtcbiAgfTsgLy8gdHlwZSBTd2FybVNldHVwID0ge1xuICAvLyAgIGFjY291bnQgOiBTdHJpbmcsXG4gIC8vICAgcGFzc3dvcmQgOiBTdHJpbmcsXG4gIC8vICAgZGF0YURpciA6IFN0cmluZyxcbiAgLy8gICBiaW5QYXRoIDogU3RyaW5nLFxuICAvLyAgIGVuc0FwaSA6IFN0cmluZyxcbiAgLy8gICBvbkRvd25sb2FkUHJvZ3Jlc3MgOiBOdW1iZXIgfj4gKCksXG4gIC8vICAgYXJjaGl2ZXMgOiBbe1xuICAvLyAgICAgYXJjaGl2ZTogU3RyaW5nLFxuICAvLyAgICAgYmluYXJ5TUQ1OiBTdHJpbmcsXG4gIC8vICAgICBhcmNoaXZlTUQ1OiBTdHJpbmdcbiAgLy8gICB9XVxuICAvLyB9XG4gIC8vIFN3YXJtU2V0dXAgfj4gUHJvbWlzZSBQcm9jZXNzXG4gIC8vICAgU3RhcnRzIHRoZSBTd2FybSBwcm9jZXNzLlxuXG5cbiAgdmFyIHN0YXJ0UHJvY2VzcyA9IGZ1bmN0aW9uIHN0YXJ0UHJvY2Vzcyhzd2FybVNldHVwKSB7XG4gICAgcmV0dXJuIG5ldyBQcm9taXNlKGZ1bmN0aW9uIChyZXNvbHZlLCByZWplY3QpIHtcbiAgICAgIHZhciBzcGF3biA9IGNoaWxkX3Byb2Nlc3Muc3Bhd247XG5cbiAgICAgIHZhciBoYXNTdHJpbmcgPSBmdW5jdGlvbiBoYXNTdHJpbmcoc3RyKSB7XG4gICAgICAgIHJldHVybiBmdW5jdGlvbiAoYnVmZmVyKSB7XG4gICAgICAgICAgcmV0dXJuICgnJyArIGJ1ZmZlcikuaW5kZXhPZihzdHIpICE9PSAtMTtcbiAgICAgICAgfTtcbiAgICAgIH07XG5cbiAgICAgIHZhciBhY2NvdW50ID0gc3dhcm1TZXR1cC5hY2NvdW50LFxuICAgICAgICAgIHBhc3N3b3JkID0gc3dhcm1TZXR1cC5wYXNzd29yZCxcbiAgICAgICAgICBkYXRhRGlyID0gc3dhcm1TZXR1cC5kYXRhRGlyLFxuICAgICAgICAgIGVuc0FwaSA9IHN3YXJtU2V0dXAuZW5zQXBpLFxuICAgICAgICAgIHByaXZhdGVLZXkgPSBzd2FybVNldHVwLnByaXZhdGVLZXk7XG4gICAgICB2YXIgU1RBUlRVUF9USU1FT1VUX1NFQ1MgPSAzO1xuICAgICAgdmFyIFdBSVRJTkdfUEFTU1dPUkQgPSAwO1xuICAgICAgdmFyIFNUQVJUSU5HID0gMTtcbiAgICAgIHZhciBMSVNURU5JTkcgPSAyO1xuICAgICAgdmFyIFBBU1NXT1JEX1BST01QVF9IT09LID0gXCJQYXNzcGhyYXNlXCI7XG4gICAgICB2YXIgTElTVEVOSU5HX0hPT0sgPSBcIlN3YXJtIGh0dHAgcHJveHkgc3RhcnRlZFwiO1xuICAgICAgdmFyIHN0YXRlID0gV0FJVElOR19QQVNTV09SRDtcbiAgICAgIHZhciBzd2FybVByb2Nlc3MgPSBzcGF3bihzd2FybVNldHVwLmJpblBhdGgsIFsnLS1ienphY2NvdW50JywgYWNjb3VudCB8fCBwcml2YXRlS2V5LCAnLS1kYXRhZGlyJywgZGF0YURpciwgJy0tZW5zLWFwaScsIGVuc0FwaV0pO1xuXG4gICAgICB2YXIgaGFuZGxlUHJvY2Vzc091dHB1dCA9IGZ1bmN0aW9uIGhhbmRsZVByb2Nlc3NPdXRwdXQoZGF0YSkge1xuICAgICAgICBpZiAoc3RhdGUgPT09IFdBSVRJTkdfUEFTU1dPUkQgJiYgaGFzU3RyaW5nKFBBU1NXT1JEX1BST01QVF9IT09LKShkYXRhKSkge1xuICAgICAgICAgIHNldFRpbWVvdXQoZnVuY3Rpb24gKCkge1xuICAgICAgICAgICAgc3RhdGUgPSBTVEFSVElORztcbiAgICAgICAgICAgIHN3YXJtUHJvY2Vzcy5zdGRpbi53cml0ZShwYXNzd29yZCArICdcXG4nKTtcbiAgICAgICAgICB9LCA1MDApO1xuICAgICAgICB9IGVsc2UgaWYgKGhhc1N0cmluZyhMSVNURU5JTkdfSE9PSykoZGF0YSkpIHtcbiAgICAgICAgICBzdGF0ZSA9IExJU1RFTklORztcbiAgICAgICAgICBjbGVhclRpbWVvdXQodGltZW91dCk7XG4gICAgICAgICAgcmVzb2x2ZShzd2FybVByb2Nlc3MpO1xuICAgICAgICB9XG4gICAgICB9O1xuXG4gICAgICBzd2FybVByb2Nlc3Muc3Rkb3V0Lm9uKCdkYXRhJywgaGFuZGxlUHJvY2Vzc091dHB1dCk7XG4gICAgICBzd2FybVByb2Nlc3Muc3RkZXJyLm9uKCdkYXRhJywgaGFuZGxlUHJvY2Vzc091dHB1dCk7IC8vc3dhcm1Qcm9jZXNzLm9uKCdjbG9zZScsICgpID0+IHNldFRpbWVvdXQocmVzdGFydCwgMjAwMCkpO1xuXG4gICAgICB2YXIgcmVzdGFydCA9IGZ1bmN0aW9uIHJlc3RhcnQoKSB7XG4gICAgICAgIHJldHVybiBzdGFydFByb2Nlc3Moc3dhcm1TZXR1cCkudGhlbihyZXNvbHZlKS5jYXRjaChyZWplY3QpO1xuICAgICAgfTtcblxuICAgICAgdmFyIGVycm9yID0gZnVuY3Rpb24gZXJyb3IoKSB7XG4gICAgICAgIHJldHVybiByZWplY3QobmV3IEVycm9yKFwiQ291bGRuJ3Qgc3RhcnQgc3dhcm0gcHJvY2Vzcy5cIikpO1xuICAgICAgfTtcblxuICAgICAgdmFyIHRpbWVvdXQgPSBzZXRUaW1lb3V0KGVycm9yLCAyMDAwMCk7XG4gICAgfSk7XG4gIH07IC8vIFByb2Nlc3Mgfj4gUHJvbWlzZSAoKVxuICAvLyAgIFN0b3BzIHRoZSBTd2FybSBwcm9jZXNzLlxuXG5cbiAgdmFyIHN0b3BQcm9jZXNzID0gZnVuY3Rpb24gc3RvcFByb2Nlc3MocHJvY2Vzcykge1xuICAgIHJldHVybiBuZXcgUHJvbWlzZShmdW5jdGlvbiAocmVzb2x2ZSwgcmVqZWN0KSB7XG4gICAgICBwcm9jZXNzLnN0ZGVyci5yZW1vdmVBbGxMaXN0ZW5lcnMoJ2RhdGEnKTtcbiAgICAgIHByb2Nlc3Muc3Rkb3V0LnJlbW92ZUFsbExpc3RlbmVycygnZGF0YScpO1xuICAgICAgcHJvY2Vzcy5zdGRpbi5yZW1vdmVBbGxMaXN0ZW5lcnMoJ2Vycm9yJyk7XG4gICAgICBwcm9jZXNzLnJlbW92ZUFsbExpc3RlbmVycygnZXJyb3InKTtcbiAgICAgIHByb2Nlc3MucmVtb3ZlQWxsTGlzdGVuZXJzKCdleGl0Jyk7XG4gICAgICBwcm9jZXNzLmtpbGwoJ1NJR0lOVCcpO1xuICAgICAgdmFyIGtpbGxUaW1lb3V0ID0gc2V0VGltZW91dChmdW5jdGlvbiAoKSB7XG4gICAgICAgIHJldHVybiBwcm9jZXNzLmtpbGwoJ1NJR0tJTEwnKTtcbiAgICAgIH0sIDgwMDApO1xuICAgICAgcHJvY2Vzcy5vbmNlKCdjbG9zZScsIGZ1bmN0aW9uICgpIHtcbiAgICAgICAgY2xlYXJUaW1lb3V0KGtpbGxUaW1lb3V0KTtcbiAgICAgICAgcmVzb2x2ZSgpO1xuICAgICAgfSk7XG4gICAgfSk7XG4gIH07IC8vIFN3YXJtU2V0dXAgLT4gKFN3YXJtQVBJIC0+IFByb21pc2UgKCkpIC0+IFByb21pc2UgKClcbiAgLy8gICBSZWNlaXZlcyBhIFN3YXJtIGNvbmZpZ3VyYXRpb24gb2JqZWN0IGFuZCBhIGNhbGxiYWNrIGZ1bmN0aW9uLiBJdCB0aGVuXG4gIC8vICAgY2hlY2tzIGlmIGEgbG9jYWwgU3dhcm0gbm9kZSBpcyBydW5uaW5nLiBJZiBubyBsb2NhbCBTd2FybSBpcyBmb3VuZCwgaXRcbiAgLy8gICBkb3dubG9hZHMgdGhlIFN3YXJtIGJpbmFyaWVzIHRvIHRoZSBkYXRhRGlyIChpZiBub3QgdGhlcmUpLCBjaGVja3N1bXMsXG4gIC8vICAgc3RhcnRzIHRoZSBTd2FybSBwcm9jZXNzIGFuZCBjYWxscyB0aGUgY2FsbGJhY2sgZnVuY3Rpb24gd2l0aCBhbiBBUElcbiAgLy8gICBvYmplY3QgdXNpbmcgdGhlIGxvY2FsIG5vZGUuIFRoYXQgY2FsbGJhY2sgbXVzdCByZXR1cm4gYSBwcm9taXNlIHdoaWNoXG4gIC8vICAgd2lsbCByZXNvbHZlIHdoZW4gaXQgaXMgZG9uZSB1c2luZyB0aGUgQVBJLCBzbyB0aGF0IHRoaXMgZnVuY3Rpb24gY2FuXG4gIC8vICAgY2xvc2UgdGhlIFN3YXJtIHByb2Nlc3MgcHJvcGVybHkuIFJldHVybnMgYSBwcm9taXNlIHRoYXQgcmVzb2x2ZXMgd2hlbiB0aGVcbiAgLy8gICB1c2VyIGlzIGRvbmUgd2l0aCB0aGUgQVBJIGFuZCB0aGUgU3dhcm0gcHJvY2VzcyBpcyBjbG9zZWQuXG4gIC8vICAgVE9ETzogY2hlY2sgaWYgU3dhcm0gcHJvY2VzcyBpcyBhbHJlYWR5IHJ1bm5pbmcgKGltcHJvdmUgYGlzQXZhaWxhYmxlYClcblxuXG4gIHZhciBsb2NhbCA9IGZ1bmN0aW9uIGxvY2FsKHN3YXJtU2V0dXApIHtcbiAgICByZXR1cm4gZnVuY3Rpb24gKHVzZUFQSSkge1xuICAgICAgcmV0dXJuIF9pc0F2YWlsYWJsZShcImh0dHA6Ly9sb2NhbGhvc3Q6ODUwMFwiKS50aGVuKGZ1bmN0aW9uIChpc0F2YWlsYWJsZSkge1xuICAgICAgICByZXR1cm4gaXNBdmFpbGFibGUgPyB1c2VBUEkoYXQoXCJodHRwOi8vbG9jYWxob3N0Ojg1MDBcIikpLnRoZW4oZnVuY3Rpb24gKCkge30pIDogZG93bmxvYWRCaW5hcnkoc3dhcm1TZXR1cC5iaW5QYXRoLCBzd2FybVNldHVwLmFyY2hpdmVzKS5vbkRhdGEoZnVuY3Rpb24gKGRhdGEpIHtcbiAgICAgICAgICByZXR1cm4gKHN3YXJtU2V0dXAub25Qcm9ncmVzcyB8fCBmdW5jdGlvbiAoKSB7fSkoZGF0YS5sZW5ndGgpO1xuICAgICAgICB9KS50aGVuKGZ1bmN0aW9uICgpIHtcbiAgICAgICAgICByZXR1cm4gc3RhcnRQcm9jZXNzKHN3YXJtU2V0dXApO1xuICAgICAgICB9KS50aGVuKGZ1bmN0aW9uIChwcm9jZXNzKSB7XG4gICAgICAgICAgcmV0dXJuIHVzZUFQSShhdChcImh0dHA6Ly9sb2NhbGhvc3Q6ODUwMFwiKSkudGhlbihmdW5jdGlvbiAoKSB7XG4gICAgICAgICAgICByZXR1cm4gcHJvY2VzcztcbiAgICAgICAgICB9KTtcbiAgICAgICAgfSkudGhlbihzdG9wUHJvY2Vzcyk7XG4gICAgICB9KTtcbiAgICB9O1xuICB9OyAvLyBTdHJpbmcgfj4gUHJvbWlzZSBCb29sXG4gIC8vICAgUmV0dXJucyB0cnVlIGlmIFN3YXJtIGlzIGF2YWlsYWJsZSBvbiBgdXJsYC5cbiAgLy8gICBQZXJmb21zIGEgdGVzdCB1cGxvYWQgdG8gZGV0ZXJtaW5lIHRoYXQuXG4gIC8vICAgVE9ETzogaW1wcm92ZSB0aGlzP1xuXG5cbiAgdmFyIF9pc0F2YWlsYWJsZSA9IGZ1bmN0aW9uIGlzQXZhaWxhYmxlKHN3YXJtVXJsKSB7XG4gICAgdmFyIHRlc3RGaWxlID0gXCJ0ZXN0XCI7XG4gICAgdmFyIHRlc3RIYXNoID0gXCJjOWE5OWM3ZDMyNmRjYzYzMTZmMzJmZTI2MjViMzExZjZkYzQ5YTE3NWU2ODc3NjgxZGVkOTMxMzdkMzU2OWU3XCI7XG4gICAgcmV0dXJuIHVwbG9hZERhdGEoc3dhcm1VcmwpKHRlc3RGaWxlKS50aGVuKGZ1bmN0aW9uIChoYXNoKSB7XG4gICAgICByZXR1cm4gaGFzaCA9PT0gdGVzdEhhc2g7XG4gICAgfSkuY2F0Y2goZnVuY3Rpb24gKCkge1xuICAgICAgcmV0dXJuIGZhbHNlO1xuICAgIH0pO1xuICB9OyAvLyBTdHJpbmcgLT4gU3RyaW5nIH4+IFByb21pc2UgQm9vbFxuICAvLyAgIFJldHVybnMgYSBQcm9taXNlIHdoaWNoIGlzIHRydWUgaWYgdGhhdCBTd2FybSBhZGRyZXNzIGlzIGEgZGlyZWN0b3J5LlxuICAvLyAgIERldGVybWluZXMgdGhhdCBieSBjaGVja2luZyB0aGF0IGl0IChpKSBpcyBhIEpTT04sIChpaSkgaGFzIGEgLmVudHJpZXMuXG4gIC8vICAgVE9ETzogaW1wcm92ZSB0aGlzP1xuXG5cbiAgdmFyIGlzRGlyZWN0b3J5ID0gZnVuY3Rpb24gaXNEaXJlY3Rvcnkoc3dhcm1VcmwpIHtcbiAgICByZXR1cm4gZnVuY3Rpb24gKGhhc2gpIHtcbiAgICAgIHJldHVybiBkb3dubG9hZERhdGEoc3dhcm1VcmwpKGhhc2gpLnRoZW4oZnVuY3Rpb24gKGRhdGEpIHtcbiAgICAgICAgdHJ5IHtcbiAgICAgICAgICByZXR1cm4gISFKU09OLnBhcnNlKHRvU3RyaW5nKGRhdGEpKS5lbnRyaWVzO1xuICAgICAgICB9IGNhdGNoIChlKSB7XG4gICAgICAgICAgcmV0dXJuIGZhbHNlO1xuICAgICAgICB9XG4gICAgICB9KTtcbiAgICB9O1xuICB9OyAvLyBVbmN1cnJpZXMgYSBmdW5jdGlvbjsgdXNlZCB0byBhbGxvdyB0aGUgZih4LHkseikgc3R5bGUgb24gZXhwb3J0cy5cblxuXG4gIHZhciB1bmN1cnJ5ID0gZnVuY3Rpb24gdW5jdXJyeShmKSB7XG4gICAgcmV0dXJuIGZ1bmN0aW9uIChhLCBiLCBjLCBkLCBlKSB7XG4gICAgICB2YXIgcDsgLy8gSGFyZGNvZGVkIGJlY2F1c2UgZWZmaWNpZW5jeSAoYGFyZ3VtZW50c2AgaXMgdmVyeSBzbG93KS5cblxuICAgICAgaWYgKHR5cGVvZiBhICE9PSBcInVuZGVmaW5lZFwiKSBwID0gZihhKTtcbiAgICAgIGlmICh0eXBlb2YgYiAhPT0gXCJ1bmRlZmluZWRcIikgcCA9IGYoYik7XG4gICAgICBpZiAodHlwZW9mIGMgIT09IFwidW5kZWZpbmVkXCIpIHAgPSBmKGMpO1xuICAgICAgaWYgKHR5cGVvZiBkICE9PSBcInVuZGVmaW5lZFwiKSBwID0gZihkKTtcbiAgICAgIGlmICh0eXBlb2YgZSAhPT0gXCJ1bmRlZmluZWRcIikgcCA9IGYoZSk7XG4gICAgICByZXR1cm4gcDtcbiAgICB9O1xuICB9OyAvLyAoKSAtPiBQcm9taXNlIEJvb2xcbiAgLy8gICBOb3Qgc3VyZSBob3cgdG8gbW9jayBTd2FybSB0byB0ZXN0IGl0IHByb3Blcmx5LiBJZGVhcz9cblxuXG4gIHZhciB0ZXN0ID0gZnVuY3Rpb24gdGVzdCgpIHtcbiAgICByZXR1cm4gUHJvbWlzZS5yZXNvbHZlKHRydWUpO1xuICB9OyAvLyBVaW50OEFycmF5IC0+IFN0cmluZ1xuXG5cbiAgdmFyIHRvU3RyaW5nID0gZnVuY3Rpb24gdG9TdHJpbmcodWludDhBcnJheSkge1xuICAgIHJldHVybiBieXRlcy50b1N0cmluZyhieXRlcy5mcm9tVWludDhBcnJheSh1aW50OEFycmF5KSk7XG4gIH07IC8vIFN0cmluZyAtPiBVaW50OEFycmF5XG5cblxuICB2YXIgZnJvbVN0cmluZyA9IGZ1bmN0aW9uIGZyb21TdHJpbmcoc3RyaW5nKSB7XG4gICAgcmV0dXJuIGJ5dGVzLnRvVWludDhBcnJheShieXRlcy5mcm9tU3RyaW5nKHN0cmluZykpO1xuICB9OyAvLyBTdHJpbmcgLT4gU3dhcm1BUElcbiAgLy8gICBGaXhlcyB0aGUgYHN3YXJtVXJsYCwgcmV0dXJuaW5nIGFuIEFQSSB3aGVyZSB5b3UgZG9uJ3QgaGF2ZSB0byBwYXNzIGl0LlxuXG5cbiAgdmFyIGF0ID0gZnVuY3Rpb24gYXQoc3dhcm1VcmwpIHtcbiAgICByZXR1cm4ge1xuICAgICAgZG93bmxvYWQ6IGZ1bmN0aW9uIGRvd25sb2FkKGhhc2gsIHBhdGgpIHtcbiAgICAgICAgcmV0dXJuIF9kb3dubG9hZChzd2FybVVybCkoaGFzaCkocGF0aCk7XG4gICAgICB9LFxuICAgICAgZG93bmxvYWREYXRhOiB1bmN1cnJ5KGRvd25sb2FkRGF0YShzd2FybVVybCkpLFxuICAgICAgZG93bmxvYWREYXRhVG9EaXNrOiB1bmN1cnJ5KGRvd25sb2FkRGF0YVRvRGlzayhzd2FybVVybCkpLFxuICAgICAgZG93bmxvYWREaXJlY3Rvcnk6IHVuY3VycnkoZG93bmxvYWREaXJlY3Rvcnkoc3dhcm1VcmwpKSxcbiAgICAgIGRvd25sb2FkRGlyZWN0b3J5VG9EaXNrOiB1bmN1cnJ5KGRvd25sb2FkRGlyZWN0b3J5VG9EaXNrKHN3YXJtVXJsKSksXG4gICAgICBkb3dubG9hZEVudHJpZXM6IHVuY3VycnkoZG93bmxvYWRFbnRyaWVzKHN3YXJtVXJsKSksXG4gICAgICBkb3dubG9hZFJvdXRlczogdW5jdXJyeShkb3dubG9hZFJvdXRlcyhzd2FybVVybCkpLFxuICAgICAgaXNBdmFpbGFibGU6IGZ1bmN0aW9uIGlzQXZhaWxhYmxlKCkge1xuICAgICAgICByZXR1cm4gX2lzQXZhaWxhYmxlKHN3YXJtVXJsKTtcbiAgICAgIH0sXG4gICAgICB1cGxvYWQ6IGZ1bmN0aW9uIHVwbG9hZChhcmcpIHtcbiAgICAgICAgcmV0dXJuIF91cGxvYWQoc3dhcm1VcmwpKGFyZyk7XG4gICAgICB9LFxuICAgICAgdXBsb2FkRGF0YTogdW5jdXJyeSh1cGxvYWREYXRhKHN3YXJtVXJsKSksXG4gICAgICB1cGxvYWRGaWxlOiB1bmN1cnJ5KHVwbG9hZEZpbGUoc3dhcm1VcmwpKSxcbiAgICAgIHVwbG9hZEZpbGVGcm9tRGlzazogdW5jdXJyeSh1cGxvYWRGaWxlKHN3YXJtVXJsKSksXG4gICAgICB1cGxvYWREYXRhRnJvbURpc2s6IHVuY3VycnkodXBsb2FkRGF0YUZyb21EaXNrKHN3YXJtVXJsKSksXG4gICAgICB1cGxvYWREaXJlY3Rvcnk6IHVuY3VycnkodXBsb2FkRGlyZWN0b3J5KHN3YXJtVXJsKSksXG4gICAgICB1cGxvYWREaXJlY3RvcnlGcm9tRGlzazogdW5jdXJyeSh1cGxvYWREaXJlY3RvcnlGcm9tRGlzayhzd2FybVVybCkpLFxuICAgICAgdXBsb2FkVG9NYW5pZmVzdDogdW5jdXJyeSh1cGxvYWRUb01hbmlmZXN0KHN3YXJtVXJsKSksXG4gICAgICBwaWNrOiBwaWNrLFxuICAgICAgaGFzaDogaGFzaCxcbiAgICAgIGZyb21TdHJpbmc6IGZyb21TdHJpbmcsXG4gICAgICB0b1N0cmluZzogdG9TdHJpbmdcbiAgICB9O1xuICB9O1xuXG4gIHJldHVybiB7XG4gICAgYXQ6IGF0LFxuICAgIGxvY2FsOiBsb2NhbCxcbiAgICBkb3dubG9hZDogX2Rvd25sb2FkLFxuICAgIGRvd25sb2FkQmluYXJ5OiBkb3dubG9hZEJpbmFyeSxcbiAgICBkb3dubG9hZERhdGE6IGRvd25sb2FkRGF0YSxcbiAgICBkb3dubG9hZERhdGFUb0Rpc2s6IGRvd25sb2FkRGF0YVRvRGlzayxcbiAgICBkb3dubG9hZERpcmVjdG9yeTogZG93bmxvYWREaXJlY3RvcnksXG4gICAgZG93bmxvYWREaXJlY3RvcnlUb0Rpc2s6IGRvd25sb2FkRGlyZWN0b3J5VG9EaXNrLFxuICAgIGRvd25sb2FkRW50cmllczogZG93bmxvYWRFbnRyaWVzLFxuICAgIGRvd25sb2FkUm91dGVzOiBkb3dubG9hZFJvdXRlcyxcbiAgICBpc0F2YWlsYWJsZTogX2lzQXZhaWxhYmxlLFxuICAgIHN0YXJ0UHJvY2Vzczogc3RhcnRQcm9jZXNzLFxuICAgIHN0b3BQcm9jZXNzOiBzdG9wUHJvY2VzcyxcbiAgICB1cGxvYWQ6IF91cGxvYWQsXG4gICAgdXBsb2FkRGF0YTogdXBsb2FkRGF0YSxcbiAgICB1cGxvYWREYXRhRnJvbURpc2s6IHVwbG9hZERhdGFGcm9tRGlzayxcbiAgICB1cGxvYWRGaWxlOiB1cGxvYWRGaWxlLFxuICAgIHVwbG9hZEZpbGVGcm9tRGlzazogdXBsb2FkRmlsZUZyb21EaXNrLFxuICAgIHVwbG9hZERpcmVjdG9yeTogdXBsb2FkRGlyZWN0b3J5LFxuICAgIHVwbG9hZERpcmVjdG9yeUZyb21EaXNrOiB1cGxvYWREaXJlY3RvcnlGcm9tRGlzayxcbiAgICB1cGxvYWRUb01hbmlmZXN0OiB1cGxvYWRUb01hbmlmZXN0LFxuICAgIHBpY2s6IHBpY2ssXG4gICAgaGFzaDogaGFzaCxcbiAgICBmcm9tU3RyaW5nOiBmcm9tU3RyaW5nLFxuICAgIHRvU3RyaW5nOiB0b1N0cmluZ1xuICB9O1xufTsiXX0=
