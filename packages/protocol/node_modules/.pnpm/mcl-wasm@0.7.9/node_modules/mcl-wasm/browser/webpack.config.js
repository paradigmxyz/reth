module.exports = {
  entry: "./src/index-browser.js",
  mode: "production",
  output: {
    path: __dirname + '/',
    library: 'mcl',
    libraryTarget: 'umd',
    filename: 'mcl.js'
  },
/*
  resolve: {
    fallback: {
      path: false,
      fs: false,
      crypto: false,
    },
  },
*/
  target: "web"
};
