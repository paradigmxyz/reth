// TODO: this is a temporary fix to hide those libraries from the browser. A
// slightly better long-term solution would be to split this file into two,
// separating the functions that are used on Node.js from the functions that
// are used only on the browser.
module.exports = ({
  fs,
  files,
  os,
  path,
  child_process,
  mimetype,
  defaultArchives,
  request,
  downloadUrl,
  bytes,
  hash,
  pick
}) => {

  // ∀ a . String -> JSON -> Map String a -o Map String a
  //   Inserts a key/val pair in an object impurely.
  const impureInsert = key => val => map =>
    (map[key] = val, map);

  // String -> JSON -> Map String JSON
  //   Merges an array of keys and an array of vals into an object.
  const toMap = keys => vals => {
    let map = {};
    for (let i = 0, l = keys.length; i < l; ++i)
      map[keys[i]] = vals[i];
    return map;
  };

  // ∀ a . Map String a -> Map String a -> Map String a
  //   Merges two maps into one.
  const merge = a => b => {
    let map = {};
    for (let key in a)
      map[key] = a[key];
    for (let key in b)
      map[key] = b[key];
    return map;
  };

  // ∀ a . [a] -> [a] -> Bool
  const equals = a => b => {
    if (a.length !== b.length) {
      return false;
    } else {
      for (let i = 0, l = a.length; i < l; ++i) {
        if (a[i] !== b[i]) return false;
      }
    }
    return true;
  }

  // String -> String -> String
  const rawUrl = swarmUrl => hash =>
    `${swarmUrl}/bzz-raw:/${hash}`

  // String -> String -> Promise Uint8Array
  //   Gets the raw contents of a Swarm hash address.
  const downloadData = swarmUrl => hash =>
    new Promise((resolve, reject) => {
      request(rawUrl(swarmUrl)(hash), {responseType: "arraybuffer"}, (err, arrayBuffer, response) => {
        if (err) {
          return reject(err);
        }
        if (response.statusCode >= 400) {
          return reject(new Error(`Error ${response.statusCode}.`));
        }
        return resolve(new Uint8Array(arrayBuffer));
      })
    });

  // type Entry = {"type": String, "hash": String}
  // type File = {"type": String, "data": Uint8Array}

  // String -> String -> Promise (Map String Entry)
  //   Solves the manifest of a Swarm address recursively.
  //   Returns a map from full paths to entries.
  const downloadEntries = swarmUrl => hash => {
    const search = hash => path => routes => {
      // Formats an entry to the Swarm.js type.
      const format = entry => ({
        type: entry.contentType,
        hash: entry.hash});

      // To download a single entry:
      //   if type is bzz-manifest, go deeper
      //   if not, add it to the routing table
      const downloadEntry = entry => {
        if (entry.path === undefined) {
          return Promise.resolve();
        } else {
          return entry.contentType === "application/bzz-manifest+json"
            ? search (entry.hash) (path + entry.path) (routes)
            : Promise.resolve (impureInsert (path + entry.path) (format(entry)) (routes));
        }
      }

      // Downloads the initial manifest and then each entry.
      return downloadData(swarmUrl)(hash)
        .then(text => JSON.parse(toString(text)).entries)
        .then(entries => Promise.all(entries.map(downloadEntry)))
        .then(() => routes);
    }

    return search (hash) ("") ({});
  }

  // String -> String -> Promise (Map String String)
  //   Same as `downloadEntries`, but returns only hashes (no types).
  const downloadRoutes = swarmUrl => hash =>
    downloadEntries(swarmUrl)(hash)
      .then(entries => toMap
        (Object.keys(entries))
        (Object.keys(entries).map(route => entries[route].hash)));

  // String -> String -> Promise (Map String File)
  //   Gets the entire directory tree in a Swarm address.
  //   Returns a promise mapping paths to file contents.
  const downloadDirectory = swarmUrl => hash =>
    downloadEntries (swarmUrl) (hash)
      .then(entries => {
        const paths = Object.keys(entries);
        const hashs = paths.map(path => entries[path].hash);
        const types = paths.map(path => entries[path].type);
        const datas = hashs.map(downloadData(swarmUrl));
        const files = datas => datas.map((data, i) => ({type: types[i], data: data}));
        return Promise.all(datas).then(datas => toMap(paths)(files(datas)));
      });

  // String -> String -> String -> Promise String
  //   Gets the raw contents of a Swarm hash address.
  //   Returns a promise with the downloaded file path.
  const downloadDataToDisk = swarmUrl => hash => filePath =>
    files.download (rawUrl(swarmUrl)(hash)) (filePath);

  // String -> String -> String -> Promise (Map String String)
  //   Gets the entire directory tree in a Swarm address.
  //   Returns a promise mapping paths to file contents.
  const downloadDirectoryToDisk = swarmUrl => hash => dirPath =>
    downloadRoutes (swarmUrl) (hash)
      .then(routingTable => {
        let downloads = [];
        for (let route in routingTable) {
          if (route.length > 0) {
            const filePath = path.join(dirPath, route);
            downloads.push(downloadDataToDisk(swarmUrl)(routingTable[route])(filePath));
          };
        };
        return Promise.all(downloads).then(() => dirPath);
      });

  // String -> Uint8Array -> Promise String
  //   Uploads raw data to Swarm.
  //   Returns a promise with the uploaded hash.
  const uploadData = swarmUrl => data =>
    new Promise((resolve, reject) => {
      const params = {
        body: typeof data === "string" ? fromString(data) : data,
        method: "POST"
      };
      request(`${swarmUrl}/bzz-raw:/`, params, (err, data) => {
        if (err) {
          return reject(err);
        }
        return resolve(data);
      });
    });

  // String -> String -> String -> File -> Promise String
  //   Uploads a file to the Swarm manifest at a given hash, under a specific
  //   route. Returns a promise containing the uploaded hash.
  //   FIXME: for some reasons Swarm-Gateways is sometimes returning
  //   error 404 (bad request), so we retry up to 3 times. Why?
  const uploadToManifest = swarmUrl => hash => route => file => {
    const attempt = n => {
      const slashRoute = route[0] === "/" ? route : "/" + route;
      const url = `${swarmUrl}/bzz:/${hash}${slashRoute}`;
      const opt = {
        method: "PUT",
        headers: {"Content-Type": file.type},
        body: file.data};
      return new Promise((resolve, reject) => {
        request(url, opt, (err, data) => {
          if (err) {
            return reject(err);
          }
          if (data.indexOf("error") !== -1) {
            return reject(data);
          }
          return resolve(data);
        });
      }).catch(e => n > 0 && attempt(n-1));
    };
    return attempt(3);
  };

  // String -> {type: String, data: Uint8Array} -> Promise String
  const uploadFile = swarmUrl => file =>
    uploadDirectory(swarmUrl)({"": file});

  // String -> String -> Promise String
  const uploadFileFromDisk = swarmUrl => filePath =>
    fs.readFile(filePath)
      .then(data => uploadFile(swarmUrl)({type: mimetype.lookup(filePath), data: data}));

  // String -> Map String File -> Promise String
  //   Uploads a directory to Swarm. The directory is
  //   represented as a map of routes and files.
  //   A default path is encoded by having a "" route.
  const uploadDirectory = swarmUrl => directory =>
    uploadData(swarmUrl)("{}")
      .then(hash => {
        const uploadRoute = route => hash => uploadToManifest(swarmUrl)(hash)(route)(directory[route]);
        const uploadToHash = (hash, route) => hash.then(uploadRoute(route));
        return Object.keys(directory).reduce(uploadToHash, Promise.resolve(hash));
      });

  // String -> Promise String
  const uploadDataFromDisk = swarmUrl => filePath =>
    fs.readFile(filePath)
      .then(uploadData(swarmUrl));

  // String -> Nullable String -> String -> Promise String
  const uploadDirectoryFromDisk = swarmUrl => defaultPath => dirPath =>
    files.directoryTree(dirPath)
      .then(fullPaths => Promise.all(fullPaths.map(path => fs.readFile(path))).then(datas => {
        const paths = fullPaths.map(path => path.slice(dirPath.length));
        const types = fullPaths.map(path => mimetype.lookup(path) || "text/plain");
        return toMap (paths) (datas.map((data, i) => ({type: types[i], data: data})));
      }))
      .then(directory => merge (defaultPath ? {"": directory[defaultPath]} : {}) (directory))
      .then(uploadDirectory(swarmUrl));

  // String -> UploadInfo -> Promise String
  //   Simplified multi-type upload which calls the correct
  //   one based on the type of the argument given.
  const upload = swarmUrl => arg => {
    // Upload raw data from browser
    if (arg.pick === "data") {
      return pick.data().then(uploadData(swarmUrl));

    // Upload a file from browser
    } else if (arg.pick === "file") {
      return pick.file().then(uploadFile(swarmUrl));

    // Upload a directory from browser
    } else if (arg.pick === "directory") {
        return pick.directory().then(uploadDirectory(swarmUrl));

    // Upload directory/file from disk
    } else if (arg.path) {
      switch (arg.kind) {
        case "data": return uploadDataFromDisk(swarmUrl)(arg.path);
        case "file": return uploadFileFromDisk(swarmUrl)(arg.path);
        case "directory": return uploadDirectoryFromDisk(swarmUrl)(arg.defaultFile)(arg.path);
      };

    // Upload UTF-8 string or raw data (buffer)
    } else if (arg.length || typeof arg === "string") {
      return uploadData(swarmUrl)(arg);

    // Upload directory with JSON
    } else if (arg instanceof Object) {
      return uploadDirectory(swarmUrl)(arg);
    }

    return Promise.reject(new Error("Bad arguments"));
  }

  // String -> String -> Nullable String -> Promise (String | Uint8Array | Map String Uint8Array)
  //   Simplified multi-type download which calls the correct function based on
  //   the type of the argument given, and on whether the Swwarm address has a
  //   directory or a file.
  const download = swarmUrl => hash => path =>
    isDirectory(swarmUrl)(hash).then(isDir => {
      if (isDir) {
        return path
          ? downloadDirectoryToDisk(swarmUrl)(hash)(path)
          : downloadDirectory(swarmUrl)(hash);
      } else {
        return path
          ? downloadDataToDisk(swarmUrl)(hash)(path)
          : downloadData(swarmUrl)(hash);
      }
    });

  // String -> Promise String
  //   Downloads the Swarm binaries into a path. Returns a promise that only
  //   resolves when the exact Swarm file is there, and verified to be correct.
  //   If it was already there to begin with, skips the download.
  const downloadBinary = (path, archives) => {
    const system = os.platform().replace("win32","windows") + "-" + (os.arch() === "x64" ? "amd64" : "386");
    const archive = (archives || defaultArchives)[system];
    const archiveUrl = downloadUrl + archive.archive + ".tar.gz";
    const archiveMD5 = archive.archiveMD5;
    const binaryMD5 = archive.binaryMD5;
    return files.safeDownloadArchived(archiveUrl)(archiveMD5)(binaryMD5)(path);
  };

  // type SwarmSetup = {
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
  const startProcess = swarmSetup => new Promise((resolve, reject) => {
    const {spawn} = child_process;

    const hasString = str => buffer => ('' + buffer).indexOf(str) !== -1;
    const {account, password, dataDir, ensApi, privateKey} = swarmSetup;

    const STARTUP_TIMEOUT_SECS = 3;
    const WAITING_PASSWORD = 0;
    const STARTING = 1;
    const LISTENING = 2;
    const PASSWORD_PROMPT_HOOK = "Passphrase";
    const LISTENING_HOOK = "Swarm http proxy started";

    let state = WAITING_PASSWORD;

    const swarmProcess = spawn(swarmSetup.binPath, [
      '--bzzaccount', account || privateKey,
      '--datadir', dataDir,
      '--ens-api', ensApi]);

    const handleProcessOutput = data => {
      if (state === WAITING_PASSWORD && hasString (PASSWORD_PROMPT_HOOK) (data)) {
        setTimeout(() => {
          state = STARTING;
          swarmProcess.stdin.write(password + '\n');
        }, 500);
      } else if (hasString (LISTENING_HOOK) (data)) {
        state = LISTENING;
        clearTimeout(timeout);
        resolve(swarmProcess);
      }
    }

    swarmProcess.stdout.on('data', handleProcessOutput);
    swarmProcess.stderr.on('data', handleProcessOutput);
    //swarmProcess.on('close', () => setTimeout(restart, 2000));

    let restart = () => startProcess(swarmSetup).then(resolve).catch(reject);
    let error = () => reject(new Error("Couldn't start swarm process."));
    let timeout = setTimeout(error, 20000);
  });

  // Process ~> Promise ()
  //   Stops the Swarm process.
  const stopProcess = process => new Promise((resolve, reject) => {
    process.stderr.removeAllListeners('data');
    process.stdout.removeAllListeners('data');
    process.stdin.removeAllListeners('error');
    process.removeAllListeners('error');
    process.removeAllListeners('exit');
    process.kill('SIGINT');

    const killTimeout = setTimeout(
      () => process.kill('SIGKILL'),
      8000);

    process.once('close', () => {
      clearTimeout(killTimeout);
      resolve();
    });
  });

  // SwarmSetup -> (SwarmAPI -> Promise ()) -> Promise ()
  //   Receives a Swarm configuration object and a callback function. It then
  //   checks if a local Swarm node is running. If no local Swarm is found, it
  //   downloads the Swarm binaries to the dataDir (if not there), checksums,
  //   starts the Swarm process and calls the callback function with an API
  //   object using the local node. That callback must return a promise which
  //   will resolve when it is done using the API, so that this function can
  //   close the Swarm process properly. Returns a promise that resolves when the
  //   user is done with the API and the Swarm process is closed.
  //   TODO: check if Swarm process is already running (improve `isAvailable`)
  const local = swarmSetup => useAPI =>
    isAvailable("http://localhost:8500").then(isAvailable =>
      isAvailable
        ? useAPI(at("http://localhost:8500")).then(() => {})
        : downloadBinary(swarmSetup.binPath, swarmSetup.archives)
          .onData(data => (swarmSetup.onProgress || (() => {}))(data.length))
          .then(() => startProcess(swarmSetup))
          .then(process => useAPI(at("http://localhost:8500")).then(() => process))
          .then(stopProcess));

  // String ~> Promise Bool
  //   Returns true if Swarm is available on `url`.
  //   Perfoms a test upload to determine that.
  //   TODO: improve this?
  const isAvailable = swarmUrl => {
    const testFile = "test";
    const testHash = "c9a99c7d326dcc6316f32fe2625b311f6dc49a175e6877681ded93137d3569e7";
    return uploadData(swarmUrl)(testFile)
      .then(hash => hash === testHash)
      .catch(() => false);
  };

  // String -> String ~> Promise Bool
  //   Returns a Promise which is true if that Swarm address is a directory.
  //   Determines that by checking that it (i) is a JSON, (ii) has a .entries.
  //   TODO: improve this?
  const isDirectory = swarmUrl => hash =>
    downloadData(swarmUrl)(hash)
      .then(data => {
        try {
          return !!JSON.parse(toString(data)).entries;
        } catch (e) {
          return false;
        }
      });

  // Uncurries a function; used to allow the f(x,y,z) style on exports.
  const uncurry = f => (a,b,c,d,e) => {
    var p;
    // Hardcoded because efficiency (`arguments` is very slow).
    if (typeof a !== "undefined") p = f(a);
    if (typeof b !== "undefined") p = f(b);
    if (typeof c !== "undefined") p = f(c);
    if (typeof d !== "undefined") p = f(d);
    if (typeof e !== "undefined") p = f(e);
    return p;
  };

  // () -> Promise Bool
  //   Not sure how to mock Swarm to test it properly. Ideas?
  const test = () => Promise.resolve(true);

  // Uint8Array -> String
  const toString = uint8Array =>
    bytes.toString(bytes.fromUint8Array(uint8Array));

  // String -> Uint8Array
  const fromString = string =>
    bytes.toUint8Array(bytes.fromString(string));

  // String -> SwarmAPI
  //   Fixes the `swarmUrl`, returning an API where you don't have to pass it.
  const at = swarmUrl => ({
    download: (hash,path) => download(swarmUrl)(hash)(path),
    downloadData: uncurry(downloadData(swarmUrl)),
    downloadDataToDisk: uncurry(downloadDataToDisk(swarmUrl)),
    downloadDirectory: uncurry(downloadDirectory(swarmUrl)),
    downloadDirectoryToDisk: uncurry(downloadDirectoryToDisk(swarmUrl)),
    downloadEntries: uncurry(downloadEntries(swarmUrl)),
    downloadRoutes: uncurry(downloadRoutes(swarmUrl)),
    isAvailable: () => isAvailable(swarmUrl),
    upload: (arg) => upload(swarmUrl)(arg),
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
  });

  return {
    at,
    local,
    download,
    downloadBinary,
    downloadData,
    downloadDataToDisk,
    downloadDirectory,
    downloadDirectoryToDisk,
    downloadEntries,
    downloadRoutes,
    isAvailable,
    startProcess,
    stopProcess,
    upload,
    uploadData,
    uploadDataFromDisk,
    uploadFile,
    uploadFileFromDisk,
    uploadDirectory,
    uploadDirectoryFromDisk,
    uploadToManifest,
    pick,
    hash,
    fromString,
    toString
  };

};
