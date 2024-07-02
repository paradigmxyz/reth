var fs = require("fs");
var childProcess = require("child_process");

var files = fs.readdirSync("test");
for (var i = 0; i < files.length; i++) {
  childProcess.execSync("node test/" + files[i], {stdio: "inherit"});
}
