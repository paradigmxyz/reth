var servify = require("./..");
var assert = require("assert");

describe("Servify", function(){
  it("must be able to create a service and call its functions", function(){

    // Creates service
    var count = 0;
    return servify.api(7171, {
      square: (x) => x * x,
      concat: (a, b) => a.concat(b),
      count: () => ++count
    }).then(() => {

      // Uses service
      var lib = servify.at("http://localhost:7171");
      return Promise.all([
        lib.square(3).then(x => assert.equal(x, 9)),
        lib.concat([1,2],[3,4]).then(x => assert.equal(JSON.stringify(x), "[1,2,3,4]")),
        lib.count().then(x => assert.equal(JSON.stringify(x), "1"))
      ]);

    }).catch(function(err){
      console.log(err);
    });
  });
});
