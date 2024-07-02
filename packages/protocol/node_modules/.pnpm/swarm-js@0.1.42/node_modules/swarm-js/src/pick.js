const picker = type => () => new Promise((resolve, reject) => {
  const fileLoader = e => {
    const directory = {};
    const totalFiles = e.target.files.length;
    let loadedFiles = 0;
    [].map.call(e.target.files, file => {
      const reader = new FileReader();
      reader.onload = e => {
        const data = new Uint8Array(e.target.result);
        if (type === "directory") {
          const path = file.webkitRelativePath;
          directory[path.slice(path.indexOf("/")+1)] = {
            type: "text/plain",
            data: data
          };
          if (++loadedFiles === totalFiles)
            resolve(directory);
        } else if (type === "file") {
          const path = file.webkitRelativePath;
          resolve({"type": mimetype.lookup(path), "data": data});
        } else {
          resolve(data);
        }
      };
      reader.readAsArrayBuffer(file);
    });
  };

  let fileInput;
  if (type === "directory") {
    fileInput = document.createElement("input");
    fileInput.addEventListener("change", fileLoader);
    fileInput.type = "file";
    fileInput.webkitdirectory = true;
    fileInput.mozdirectory = true;
    fileInput.msdirectory = true;
    fileInput.odirectory = true;
    fileInput.directory = true;
  } else {
    fileInput = document.createElement("input");
    fileInput.addEventListener("change", fileLoader);
    fileInput.type = "file";
  };

  const mouseEvent = document.createEvent("MouseEvents");
  mouseEvent.initEvent("click", true, false);
  fileInput.dispatchEvent(mouseEvent);
});

module.exports = {
  data: picker("data"),
  file: picker("file"),
  directory: picker("directory"),
}
