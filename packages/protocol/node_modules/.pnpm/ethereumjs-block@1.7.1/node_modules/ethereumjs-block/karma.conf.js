process.env.ethTest = 'BasicTests'

module.exports = function (config) {
  config.set({
    browserNoActivityTimeout: 60000,
    frameworks: ['browserify', 'detectBrowsers', 'tap'],
    files: [
      './tests/difficulty.js'
    ],
    preprocessors: {
      'tests/*.js': ['browserify', 'env']
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
    browserify: {
      'transform': [
        [
          'babelify',
          {
            'presets': [
              'env'
            ]
          }
        ]
      ]
    },
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
