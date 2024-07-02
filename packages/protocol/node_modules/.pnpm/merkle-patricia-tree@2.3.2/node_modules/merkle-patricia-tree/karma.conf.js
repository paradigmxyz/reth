process.env.ethTest = 'TrieTests'

module.exports = function (config) {
  config.set({
    browserNoActivityTimeout: 120000,
    frameworks: ['browserify', 'detectBrowsers', 'tap'],
    files: [
      './test/*.js'
    ],
    preprocessors: {
      'test/*.js': ['browserify', 'env']
    },
    singleRun: true,
    plugins: [
      'karma-browserify',
      'karma-chrome-launcher',
      'karma-env-preprocessor',
      'karma-tap',
      'karma-firefox-launcher',
      'karma-detect-browsers'
    ],
    detectBrowsers: {
      enabled: true,
      usePhantomJS: false,
      postDetection: function (availableBrowser) {
        if (process.env.TRAVIS) {
          return ['Firefox']
        }

        var browsers = ['Chrome', 'Firefox']
        return browsers.filter(function (browser) {
          return availableBrowser.indexOf(browser) !== -1
        })
      }
    }
  })
}
