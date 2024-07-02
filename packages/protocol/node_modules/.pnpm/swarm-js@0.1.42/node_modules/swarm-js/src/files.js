// This module implements some file download utils. Its most important export
// is `safeDownloadTargzFile`, which, given a file, its md5, a tar.gz url, its
// md5 and a path, returns a Promise that will only resolve once the exact file
// you expect is available on that path.

const Q = require("bluebird");
const assert = require("assert");
const crypto = require("crypto");
const fs = require("fs-extra");
const got = require("got");
const mkdirp = require("mkdirp-promise");
const path = require("path");
const tar = require("tar");

// String -> String ~> Promise String
//   Downloads a file from an url to a path.
//   Returns a promise containing the path.
const download = url => filePath => {
  const promise = Q.resolve(mkdirp(path.dirname(filePath))).then(() =>
    new Q((resolve, reject) => {
      const writeStream = fs.createWriteStream(filePath);
      const downloadStream = got.stream(url);
      downloadStream.on("end", () => resolve(filePath));
      downloadStream.on("data", chunk => promise.onDataCallback(chunk));
      downloadStream.on("error", reject);
      downloadStream.pipe(writeStream);
    }));
  promise.onDataCallback = () => {};
  promise.onData = callback => {
    promise.onDataCallback = callback || (() => {});
    return promise;
  }
  return promise;
};

// String -> String ~> Promise String
//   Hashes a file using the given algorithm (ex: "md5").
//   Returns a promise containing the hashed string.
const hash = algorithm => path =>
  new Q((resolve, reject) => {
    const readStream = fs.ReadStream(path);
    const hash = crypto.createHash(algorithm);
    readStream.on("data", d => hash.update(d));
    readStream.on("end", () => resolve(hash.digest("hex")));
    readStream.on("error", reject);
  });

// String -> String ~> Promise ()
//   Asserts a file matches this md5 hash.
//   Returns a promise containing its path.
const checksum = fileHash => path =>
  hash("md5")(path)
    .then(actualHash => actualHash === fileHash)
    .then(assert)
    .then(() => path);

// String ~> String ~> String ~> Promise String
//   Downloads a file to a directory, check.
//   Checks if the md5 hash matches.
//   Returns a promise containing the path.
const downloadAndCheck = url => path => fileHash =>
  download(url)(path)
    .then(checksum(fileHash));

// String -> String ~> Promise String
//   TODO: work for zip and other types
const extract = fromPath => toPath =>
  tar.x(fromPath, toPath)
    .then(() => toPath);

// String ~> Promise String
//   Reads a file as an UTF8 string.
//   Returns a promise containing that string.
const readUTF8 = path =>
  fs.readFile (path, {encoding: "utf8"});

// String ~> Promise Bool
const isDirectory = path =>
  fs.exists(path)
    .then(assert)
    .then(() => fs.lstat(path))
    .then(stats => stats.isDirectory())
    .catch(() => false);

// String -> Promise String
const directoryTree = dirPath => {
  let paths = [];
  const search = dirPath =>
    isDirectory(dirPath).then(isDir => {
      if (isDir) {
        const searchOnDir = dir => search (path.join(dirPath, dir));
        return Q.all(Q.map(fs.readdir(dirPath), searchOnDir));
      } else {
        paths.push(dirPath);
      };
    });
  return Q.all(search (dirPath)).then(() => paths);
}

// Regex -> String ~> Promise (Array String)
const search = regex => dirPath => 
  directoryTree(dirPath)
    .then(tree => tree.filter(path => regex.test(path)));

// String -> String -> String -> String ~> Promise String
//   Downloads a file inside a tar.gz and places it at `filePath`.
//   Checks the md5 hash of the tar before extracting it.
//   Checks the md5 hash of the file after extracting it.
//   If all is OK, returns a promise containing the file path.
const safeDownloadArchived = url => archiveHash => fileHash => filePath => {
  const fileDir = path.dirname(filePath);
  const fileName = path.basename(filePath);
  const archivePath = path.join(fileDir, ".swarm_downloads/files.tar.gz");
  const archiveDir = path.dirname(archivePath);
  const promise = Q.resolve(mkdirp(archiveDir))
    .then(() => checksum (fileHash) (filePath))
    .then(() => filePath)
    .catch(() => fs.exists(archiveDir)
      .then(exists => !exists ? fs.mkdir(archiveDir) : null)
      .then(() => download (url)(archivePath).onData(promise.onDataCallback))
      .then(() => hash("md5")(archivePath))
      .then(() => archiveHash ? checksum(archiveHash)(archivePath) : null)
      .then(() => extract (archivePath) (archiveDir))
      .then(() => search (new RegExp(fileName+"$")) (archiveDir))
      .then(fp => fs.rename (fp[0], filePath))
      .then(() => fs.unlink (archivePath))
      .then(() => fileHash ? checksum(fileHash)(filePath) : null)
      .then(() => filePath));
  promise.onDataCallback = () => {};
  promise.onData = callback => {
    promise.onDataCallback = callback || (() => {});
    return promise;
  };
  return promise;
};

// String -> String ~> Promise String
//   Like `safeDownloadArchivedFile`, but without the checksums.
const downloadArchived = url => path =>
  safeDownloadArchived (url) (null) (null) (path);

// () => Promise Bool
//   Tests the implementation by downloading a predetermined tar.gz
//   from a mocked HTTP-server into a mocked filesystem. Does some
//   redundancy tests such as checking the file constents and double
//   checking its MD5 hash.
//   Returns a promise containing a boolean, true if tests passed.
const test = () => {
  const filePath = "/swarm/foo.txt";
  const fileHash = "d3b07384d113edec49eaa6238ad5ff00";
  const archiveUrl = "http://localhost:12534";
  const archiveHash = "7fa45f946bb2a696bdd9972e0fbceac2";
  const archiveData = new Buffer([
    0x1f,0x8b,0x08,0x00,0xf1,0x34,0xaf,0x58,0x00,0x03,0xed,0xcf,0x3d,0x0e,0x83,0x30,
    0x0c,0x86,0x61,0x66,0x4e,0xe1,0x13,0x54,0xce,0x0f,0xc9,0x79,0x58,0xb2,0x46,0x82,
    0x14,0x71,0x7c,0xd2,0x06,0x31,0x52,0x75,0x40,0x08,0xe9,0x7d,0x96,0x4f,0x96,0x3d,
    0x7c,0x4e,0x39,0xbf,0xca,0x5a,0xba,0x2b,0xa9,0x6a,0xf0,0x5e,0x3e,0x19,0xc3,0xf0,
    0x4d,0xb5,0x6d,0xde,0x79,0x31,0x4e,0x07,0x17,0x9c,0xb5,0x31,0x8a,0x1a,0xab,0xc6,
    0x77,0xa2,0x97,0xb6,0xda,0xbd,0xe7,0x32,0x4e,0xb5,0xca,0xf2,0xe3,0xae,0x9e,0xa5,
    0x74,0xb2,0x6f,0x8f,0xc8,0x91,0x0f,0x91,0x72,0xee,0xef,0xee,0x00,0x00,0x00,0x00,
    0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0xf8,0xdf,0x06,0xb3,0x2a,0xda,
    0xed,0x00,0x28,0x00,0x00]);

  const crypto = require("crypto");
  const fsMock = require("mock-fs")({"/swarm":{}});
  const httpMock = require("http")
    .createServer((_,res) => res.end(archiveData))
    .listen(12534);

  return safeDownloadArchived (archiveUrl) (archiveHash) (fileHash) (filePath)
    .then(checksum (fileHash))
    .then(readUTF8)
    .then(text => text === "foo\n")
    .then(assert)
    .then(() => safeDownloadArchived (archiveUrl) (archiveHash) (fileHash) (filePath))
    .then(() => true)
    .catch(false)
    .finally(() => httpMock.close());
};

module.exports = {
  download,
  hash,
  checksum,
  downloadAndCheck,
  extract,
  readUTF8,
  safeDownloadArchived,
  directoryTree,
  downloadArchived,
  search,
  test
};
