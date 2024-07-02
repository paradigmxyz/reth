const fs = require('fs')
const path = require('path')

const getLocFromIndex = (text, index) => {
  let line = 1
  let column = 0
  let i = 0
  while (i < index) {
    if (text[i] === '\n') {
      line++
      column = 0
    } else {
      column++
    }
    i++
  }

  return { line, column }
}

const walkSync = (dir, filelist = []) => {
  fs.readdirSync(dir).forEach((file) => {
    filelist = fs.statSync(path.join(dir, file)).isDirectory()
      ? walkSync(path.join(dir, file), filelist)
      : filelist.concat(path.join(dir, file))
  })
  return filelist
}

module.exports = {
  getLocFromIndex,
  walkSync,
}
