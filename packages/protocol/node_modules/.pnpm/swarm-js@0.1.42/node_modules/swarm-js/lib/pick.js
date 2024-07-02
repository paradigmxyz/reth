var picker = function picker(type) {
  return function () {
    return new Promise(function (resolve, reject) {
      var fileLoader = function fileLoader(e) {
        var directory = {};
        var totalFiles = e.target.files.length;
        var loadedFiles = 0;
        [].map.call(e.target.files, function (file) {
          var reader = new FileReader();

          reader.onload = function (e) {
            var data = new Uint8Array(e.target.result);

            if (type === "directory") {
              var path = file.webkitRelativePath;
              directory[path.slice(path.indexOf("/") + 1)] = {
                type: "text/plain",
                data: data
              };
              if (++loadedFiles === totalFiles) resolve(directory);
            } else if (type === "file") {
              var _path = file.webkitRelativePath;
              resolve({
                "type": mimetype.lookup(_path),
                "data": data
              });
            } else {
              resolve(data);
            }
          };

          reader.readAsArrayBuffer(file);
        });
      };

      var fileInput;

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
      }

      ;
      var mouseEvent = document.createEvent("MouseEvents");
      mouseEvent.initEvent("click", true, false);
      fileInput.dispatchEvent(mouseEvent);
    });
  };
};

module.exports = {
  data: picker("data"),
  file: picker("file"),
  directory: picker("directory")
};