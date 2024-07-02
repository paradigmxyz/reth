// This module implements some file download utils. Its most important export
// is `safeDownloadTargzFile`, which, given a file, its md5, a tar.gz url, its
// md5 and a path, returns a Promise that will only resolve once the exact file
// you expect is available on that path.
var Q = require("bluebird");

var assert = require("assert");

var crypto = require("crypto");

var fs = require("fs-extra");

var got = require("got");

var mkdirp = require("mkdirp-promise");

var path = require("path");

var tar = require("tar"); // String -> String ~> Promise String
//   Downloads a file from an url to a path.
//   Returns a promise containing the path.


var download = function download(url) {
  return function (filePath) {
    var promise = Q.resolve(mkdirp(path.dirname(filePath))).then(function () {
      return new Q(function (resolve, reject) {
        var writeStream = fs.createWriteStream(filePath);
        var downloadStream = got.stream(url);
        downloadStream.on("end", function () {
          return resolve(filePath);
        });
        downloadStream.on("data", function (chunk) {
          return promise.onDataCallback(chunk);
        });
        downloadStream.on("error", reject);
        downloadStream.pipe(writeStream);
      });
    });

    promise.onDataCallback = function () {};

    promise.onData = function (callback) {
      promise.onDataCallback = callback || function () {};

      return promise;
    };

    return promise;
  };
}; // String -> String ~> Promise String
//   Hashes a file using the given algorithm (ex: "md5").
//   Returns a promise containing the hashed string.


var hash = function hash(algorithm) {
  return function (path) {
    return new Q(function (resolve, reject) {
      var readStream = fs.ReadStream(path);
      var hash = crypto.createHash(algorithm);
      readStream.on("data", function (d) {
        return hash.update(d);
      });
      readStream.on("end", function () {
        return resolve(hash.digest("hex"));
      });
      readStream.on("error", reject);
    });
  };
}; // String -> String ~> Promise ()
//   Asserts a file matches this md5 hash.
//   Returns a promise containing its path.


var checksum = function checksum(fileHash) {
  return function (path) {
    return hash("md5")(path).then(function (actualHash) {
      return actualHash === fileHash;
    }).then(assert).then(function () {
      return path;
    });
  };
}; // String ~> String ~> String ~> Promise String
//   Downloads a file to a directory, check.
//   Checks if the md5 hash matches.
//   Returns a promise containing the path.


var downloadAndCheck = function downloadAndCheck(url) {
  return function (path) {
    return function (fileHash) {
      return download(url)(path).then(checksum(fileHash));
    };
  };
}; // String -> String ~> Promise String
//   TODO: work for zip and other types


var extract = function extract(fromPath) {
  return function (toPath) {
    return tar.x(fromPath, toPath).then(function () {
      return toPath;
    });
  };
}; // String ~> Promise String
//   Reads a file as an UTF8 string.
//   Returns a promise containing that string.


var readUTF8 = function readUTF8(path) {
  return fs.readFile(path, {
    encoding: "utf8"
  });
}; // String ~> Promise Bool


var isDirectory = function isDirectory(path) {
  return fs.exists(path).then(assert).then(function () {
    return fs.lstat(path);
  }).then(function (stats) {
    return stats.isDirectory();
  })["catch"](function () {
    return false;
  });
}; // String -> Promise String


var directoryTree = function directoryTree(dirPath) {
  var paths = [];

  var search = function search(dirPath) {
    return isDirectory(dirPath).then(function (isDir) {
      if (isDir) {
        var searchOnDir = function searchOnDir(dir) {
          return search(path.join(dirPath, dir));
        };

        return Q.all(Q.map(fs.readdir(dirPath), searchOnDir));
      } else {
        paths.push(dirPath);
      }

      ;
    });
  };

  return Q.all(search(dirPath)).then(function () {
    return paths;
  });
}; // Regex -> String ~> Promise (Array String)


var search = function search(regex) {
  return function (dirPath) {
    return directoryTree(dirPath).then(function (tree) {
      return tree.filter(function (path) {
        return regex.test(path);
      });
    });
  };
}; // String -> String -> String -> String ~> Promise String
//   Downloads a file inside a tar.gz and places it at `filePath`.
//   Checks the md5 hash of the tar before extracting it.
//   Checks the md5 hash of the file after extracting it.
//   If all is OK, returns a promise containing the file path.


var safeDownloadArchived = function safeDownloadArchived(url) {
  return function (archiveHash) {
    return function (fileHash) {
      return function (filePath) {
        var fileDir = path.dirname(filePath);
        var fileName = path.basename(filePath);
        var archivePath = path.join(fileDir, ".swarm_downloads/files.tar.gz");
        var archiveDir = path.dirname(archivePath);
        var promise = Q.resolve(mkdirp(archiveDir)).then(function () {
          return checksum(fileHash)(filePath);
        }).then(function () {
          return filePath;
        })["catch"](function () {
          return fs.exists(archiveDir).then(function (exists) {
            return !exists ? fs.mkdir(archiveDir) : null;
          }).then(function () {
            return download(url)(archivePath).onData(promise.onDataCallback);
          }).then(function () {
            return hash("md5")(archivePath);
          }).then(function () {
            return archiveHash ? checksum(archiveHash)(archivePath) : null;
          }).then(function () {
            return extract(archivePath)(archiveDir);
          }).then(function () {
            return search(new RegExp(fileName + "$"))(archiveDir);
          }).then(function (fp) {
            return fs.rename(fp[0], filePath);
          }).then(function () {
            return fs.unlink(archivePath);
          }).then(function () {
            return fileHash ? checksum(fileHash)(filePath) : null;
          }).then(function () {
            return filePath;
          });
        });

        promise.onDataCallback = function () {};

        promise.onData = function (callback) {
          promise.onDataCallback = callback || function () {};

          return promise;
        };

        return promise;
      };
    };
  };
}; // String -> String ~> Promise String
//   Like `safeDownloadArchivedFile`, but without the checksums.


var downloadArchived = function downloadArchived(url) {
  return function (path) {
    return safeDownloadArchived(url)(null)(null)(path);
  };
}; // () => Promise Bool
//   Tests the implementation by downloading a predetermined tar.gz
//   from a mocked HTTP-server into a mocked filesystem. Does some
//   redundancy tests such as checking the file constents and double
//   checking its MD5 hash.
//   Returns a promise containing a boolean, true if tests passed.


var test = function test() {
  var filePath = "/swarm/foo.txt";
  var fileHash = "d3b07384d113edec49eaa6238ad5ff00";
  var archiveUrl = "http://localhost:12534";
  var archiveHash = "7fa45f946bb2a696bdd9972e0fbceac2";
  var archiveData = new Buffer([0x1f, 0x8b, 0x08, 0x00, 0xf1, 0x34, 0xaf, 0x58, 0x00, 0x03, 0xed, 0xcf, 0x3d, 0x0e, 0x83, 0x30, 0x0c, 0x86, 0x61, 0x66, 0x4e, 0xe1, 0x13, 0x54, 0xce, 0x0f, 0xc9, 0x79, 0x58, 0xb2, 0x46, 0x82, 0x14, 0x71, 0x7c, 0xd2, 0x06, 0x31, 0x52, 0x75, 0x40, 0x08, 0xe9, 0x7d, 0x96, 0x4f, 0x96, 0x3d, 0x7c, 0x4e, 0x39, 0xbf, 0xca, 0x5a, 0xba, 0x2b, 0xa9, 0x6a, 0xf0, 0x5e, 0x3e, 0x19, 0xc3, 0xf0, 0x4d, 0xb5, 0x6d, 0xde, 0x79, 0x31, 0x4e, 0x07, 0x17, 0x9c, 0xb5, 0x31, 0x8a, 0x1a, 0xab, 0xc6, 0x77, 0xa2, 0x97, 0xb6, 0xda, 0xbd, 0xe7, 0x32, 0x4e, 0xb5, 0xca, 0xf2, 0xe3, 0xae, 0x9e, 0xa5, 0x74, 0xb2, 0x6f, 0x8f, 0xc8, 0x91, 0x0f, 0x91, 0x72, 0xee, 0xef, 0xee, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf8, 0xdf, 0x06, 0xb3, 0x2a, 0xda, 0xed, 0x00, 0x28, 0x00, 0x00]);

  var crypto = require("crypto");

  var fsMock = require("mock-fs")({
    "/swarm": {}
  });

  var httpMock = require("http").createServer(function (_, res) {
    return res.end(archiveData);
  }).listen(12534);

  return safeDownloadArchived(archiveUrl)(archiveHash)(fileHash)(filePath).then(checksum(fileHash)).then(readUTF8).then(function (text) {
    return text === "foo\n";
  }).then(assert).then(function () {
    return safeDownloadArchived(archiveUrl)(archiveHash)(fileHash)(filePath);
  }).then(function () {
    return true;
  })["catch"](false)["finally"](function () {
    return httpMock.close();
  });
};

module.exports = {
  download: download,
  hash: hash,
  checksum: checksum,
  downloadAndCheck: downloadAndCheck,
  extract: extract,
  readUTF8: readUTF8,
  safeDownloadArchived: safeDownloadArchived,
  directoryTree: directoryTree,
  downloadArchived: downloadArchived,
  search: search,
  test: test
};