## Disclaimer

**This library isn't activelly maintained as I moved on to other things. If you'd like to maintain it, please let me know. For now, I think I can point to Erebos: https://erebos.js.org/**


## Swarm.js

This library allows you to interact with the Swarm network from JavaScript.

### Getting started

1. Install

    ```bash
    npm install swarm-js
    ```

2. Import

    ```javascript
    // Loads the Swarm API pointing to the official gateway
    const swarm = require("swarm-js").at("http://swarm-gateways.net");
    ```

### Examples

#### Uploads

- With JSON:

    - Raw data:

        ```javascript
        const file = "test file"; // could also be an Uint8Array of binary data
        swarm.upload(file).then(hash => {
          console.log("Uploaded file. Address:", hash);
        })
        ```

    - Directory:

        To upload a directory, just call `swarm.upload(directory)`, where directory is an object mapping paths to entries, those containing a mime-type and the data (Uint8Array or UTF-8 String).

        ```javascript
        const dir = {
          "/foo.txt": {type: "text/plain", data: "file 0"},
          "/bar.txt": {type: "text/plain", data: "file 1"}
        };
        swarm.upload(dir).then(hash => {
          console.log("Uploaded directory. Address:", hash);
        });
        ```

- From disk:

    - On Node.js:

        ```javascript
        swarm.upload({
          path: "/path/to/thing",      // path to data / file / directory
          kind: "directory",           // could also be "file" or "data"
          defaultFile: "/index.html"}) // optional, and only for kind === "directory"
          .then(console.log)
          .catch(console.log);
        ```

    - On browsers:

        ```javascript
        // only works inside an event
        document.onClick = function() {
          swarm.upload({pick: "file"}) // could also be "directory" or "data"
            .then(alert);
        };
        ```

#### Downloads

- With JSON:

    - Raw data:

        ```javascript
        const fileHash = "a5c10851ef054c268a2438f10a21f6efe3dc3dcdcc2ea0e6a1a7a38bf8c91e23";
        swarm.download(fileHash).then(array => {
          console.log("Downloaded file:", swarm.toString(array));
        });
        ```

    - Directory:

        ```javascript
        const dirHash = "7e980476df218c05ecfcb0a2ca73597193a34c5a9d6da84d54e295ecd8e0c641";
        swarm.download(dirHash).then(dir => {
          console.log("Downloaded directory:");
          for (let path in dir) {
            console.log("-", path, ":", dir[path].data.toString());
          }
        });
        ```

- To disk:

    - On Node.js:

        ```javascript
        swarm.download("DAPP_HASH", "/target/dir")
          .then(path => console.log(`Downloaded DApp to ${path}.`))
          .catch(console.log);
        ```

    - On browser:

        (Just link the Swarm URL.)

#### SwarmHash

```javascript
console.log(swarm.hash("unicode string áéíóú λ"));
console.log(swarm.hash("0x41414141"));
console.log(swarm.hash([65, 65, 65, 65]));
console.log(swarm.hash(new Uint8Array([65, 65, 65, 65])));
```

### More

For more examples, check out [examples](/examples).
