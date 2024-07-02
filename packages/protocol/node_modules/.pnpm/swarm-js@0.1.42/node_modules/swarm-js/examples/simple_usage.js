const swarm = require("./..").at("http://swarm-gateways.net");

(async () => {
  try {
    // Uploading raw data
    const file = "this is a test";
    const fileHash = await swarm.upload(new Buffer(file))
    console.log("Uploaded file. SwarmHash:", fileHash);

    //// Downloading raw data
    const fileBuffer = await swarm.download(fileHash);
    console.log("Downloaded file. Contents:", swarm.toString(fileBuffer));

    // Uploading directory
    const dir = {
      "/foo.txt": {type: "text/plain", data: "this is foo.txt"},
      "/bar.txt": {type: "text/plain", data: "this is bar.txt"}
    };
    const dirHash = await swarm.upload(dir);
    console.log("Uploaded directory. SwarmHash:", dirHash);

    //// Downloaading a directory
    const dirObj = await swarm.download(dirHash);
    console.log("Downloaded directory. Contents:");
    for (let path in dirObj)
      console.log("-", path, ":", swarm.toString(dirObj[path].data));
  } catch (e) {
    console.log(e);
  }

})();
