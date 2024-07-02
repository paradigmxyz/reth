function applyFixes(fixes, inputSrc) {
  if (fixes.length === 0) {
    return {
      fixed: false,
    }
  }

  let output = ''

  let i = 0
  let fixIndex = 0

  while (i < inputSrc.length) {
    if (fixIndex < fixes.length) {
      output += inputSrc.slice(i, fixes[fixIndex].range[0])
      output += fixes[fixIndex].text
      i = fixes[fixIndex].range[1] + 1
      fixIndex += 1
    } else {
      output += inputSrc.slice(i)
      break
    }
  }

  return {
    fixed: true,
    output,
  }
}

module.exports = applyFixes
