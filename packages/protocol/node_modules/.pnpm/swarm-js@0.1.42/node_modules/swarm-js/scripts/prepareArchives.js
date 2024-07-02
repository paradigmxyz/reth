const Q = require("bluebird");
const decompress = require("decompress");
const files = require("./../src/files.js");
const fs = require("fs-extra");
const got = require("got");
const path = require("path");
const tar = require("tar");
const xml = require("xml2js");

// Downloads list of Geth releases
console.log("Retrieving list of Geth releases from https://gethstore.blob.core.windows.net.");
got("https://gethstore.blob.core.windows.net/builds?restype=container&comp=list")

  // Parses XML
  .then((res) => new Q((resolve, reject) => {
    console.log("Parsing XML.");
    xml.parseString(res.body, (err, res) => {
      if (err) {
        reject();
      }
      else {
        resolve(res.EnumerationResults.Blobs[0].Blob);
      }
    });
  }))

  // Formats data
  .then(blobs => {
    console.log("Formatting blobs.");
    return blobs
      .filter((file) => {
        const isStable = !/unstable/.test(file.Name[0]);
        const hasSwarm = /alltools/.test(file.Name[0]);
        const isArchive = !/\.asc$/.test(file.Name[0]);
        return isStable && hasSwarm && isArchive;
      })
      .map((file) => {
        const gethArchiveName = file.Name[0];
        const parts = gethArchiveName.split("-");
        const props = file.Properties[0];
        const swarmArchiveExt = parts[2] === "windows" ? ".exe" : "";
        const swarmArchiveName = `swarm-${parts[2]}-${parts[3]}-${parts[4]}${swarmArchiveExt}`;
        return {
          date: new Date(props["Last-Modified"]),
          hash: props["Content-MD5"][0],
          os: parts[2],
          arch: parts[3],
          version: parts[4],
          gethArchiveName: gethArchiveName,
          gethArchivePath: path.join(process.cwd(), "tmp_downloads", gethArchiveName),
          gethArchiveUrl: "https://gethstore.blob.core.windows.net/builds/" + gethArchiveName,
          gethFilesPath: path.join(process.cwd(), "tmp_downloads", gethArchiveName.replace(/(\.zip|\.tar\.gz)/,"")),
          swarmArchiveName: swarmArchiveName,
          swarmBinaryDir: path.join(process.cwd(), "tmp_downloads", swarmArchiveName),
          swarmBinaryPath: path.join(process.cwd(), "tmp_downloads", swarmArchiveName, "swarm" + swarmArchiveExt),
          swarmArchivePath: path.join(process.cwd(), "archives", swarmArchiveName + ".tar.gz"),
        }
      })
      .sort((a, b) => a.date - b.date)
  })

  // Downloads Geth archives
  .then(binaries => {
    console.log("Downloading archives.");
    if (!fs.existsSync("archives"))
      fs.mkdirSync(path.join(process.cwd(), "archives"));
    let archives = {};
    return Q.all(binaries
      .filter(bin => bin.version === binaries[binaries.length - 1].version)
      .map(bin => {
        const archive = archives[bin.os + "-" + bin.arch] = {
          archive: bin.swarmArchiveName
        };
        console.log("Downloading " + bin.gethArchivePath + ".");
        return files.download(bin.gethArchiveUrl)(bin.gethArchivePath)
          .then(path => (console.log("Downloaded " + path + "."), path))
          .then(path => decompress(path, "tmp_downloads"))
          .then(() => !fs.existsSync(bin.swarmBinaryDir) && fs.mkdir(bin.swarmBinaryDir))
          .then(() => files.search(/swarm(.exe|)$/)(bin.gethFilesPath))
          .then(([swarmPath]) => fs.rename(swarmPath, bin.swarmBinaryPath))
          .then(() => files.hash("md5")(bin.swarmBinaryPath))
          .then(binaryMD5 => archive.binaryMD5 = binaryMD5)
          .then(() => tar.c({gzip: true, file: bin.swarmArchivePath}, bin.swarmBinaryDir))
          .then(() => files.hash("md5")(bin.swarmArchivePath))
          .then(archiveMD5 => archive.archiveMD5 = archiveMD5)
          .catch(e => console.log(e));
      }))
      .then(() => fs.writeFileSync("archives/archives.json", JSON.stringify(archives, null, 2)));
  })

  // Finishes
  .then(() => {
    fs.removeSync(path.join(process.cwd(), "tmp_downloads"));
    console.log("Done.")
  })
  .catch(e => console.log(e));
