const estimateGas = require("./guestimation");
const binSearch = require("./binSearch");

module.exports = async(generateVM, runArgs, callback) => {
  const vm = generateVM();
  estimateGas(vm, runArgs, async(err, result) => {
    if (err) {
      callback(err);
      return;
    }

    await binSearch(generateVM, runArgs, result, callback);
  });
};
