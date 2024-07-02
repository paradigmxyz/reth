'use strict';

module.exports = function(grunt) {
  var files = ['Gruntfile.js', 'package.json', 'index.js', 'lib/*.js', 'test/*.js'];

  grunt.initConfig({
    jshint: {
      // Pre-test
      sloppy: {
        options: {
          curly: true,
          eqeqeq: true,
          eqnull: true,
          globalstrict: true,
          node: true,
          camelcase: true,
          newcap: true,
          proto: true
        },
        files: {
          src: files
        }
      },
      // Post-test
      strict: {
        options: {
          curly: true,
          eqeqeq: true,
          eqnull: true,
          globalstrict: true,
          node: true,
          camelcase: true,
          indent: 2,
          immed: true,
          latedef: 'nofunc',
          newcap: true,
          quotmark: true,
          undef: true,
          unused: true,
          trailing: true
        },
        files: {
          src: files
        }
      }
    },
    mochaTest: {
      options: {
        globals: ['should']
      },
      all: { src: ['test/**/*.js'] }
    },
    watch: {
      files: ['**/*.js', '!**/node_modules/**'],
      tasks: ['test']
    }
  });

  // npm tasks
  grunt.loadNpmTasks('grunt-contrib-jshint');
  grunt.loadNpmTasks('grunt-mocha-test');
  grunt.loadNpmTasks('grunt-contrib-watch');

  // Default task.
  grunt.registerTask('test', ['jshint:sloppy', 'mochaTest', 'jshint:strict']);
  grunt.registerTask('default', ['test', 'watch']);
};
