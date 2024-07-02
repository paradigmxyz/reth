module.exports = (function(libs){
  var express, req;
  if (!process.browser) {
    var express = libs["express"];
    var cors = libs["cors"];
    var rq = libs["request"];
    var bodyParser = libs["body-parser"];
  } else {
    var rq = libs["xhr"];
  };

  function api(port, api){
    return new Promise(function(resolve, reject){
      var app = express();
      app.use(bodyParser.json());
      app.use(cors());
      app.use(function (error, req, res, next){
          if (error.message === "invalid json")
            res.send("null");
          else next();
      });
      function callFunc(req, res){
        try {
          var url = req.url;
          var parens = url.indexOf("(");
          if (parens !== -1){
            var name = url.slice(1, parens);
            var args = JSON.parse("["+decodeURIComponent(url.slice(parens+1, -1))+"]");
          } else {
            var name = url.slice(1);
            var args = [req.body];
          }
          var result = api[name].apply(null, args);
          if (result.then)
            result.then(result => res.json(result));
          else
            res.json(result);
        } catch(e) {
          res.send("null");
        };
      };
      app.get("*", callFunc);
      app.post("*", callFunc);
      app.listen(port, resolve).on("error", reject);
    });
  };

  function at(url){
    return new Proxy({}, {
      get: function(target, func){
        return function(){
          var args = [].slice.call(arguments);
          return new Promise(function(resolve, reject){
            rq.post({uri: url+"/"+func+"("+JSON.stringify(args).slice(1,-1)+")"}, function(err, res, body){
              if (err) reject(err);
              else resolve(JSON.parse(body));
            });
          });
        };
      }
    });
  };

  return {
    api: api,
    at: at
  };
});
