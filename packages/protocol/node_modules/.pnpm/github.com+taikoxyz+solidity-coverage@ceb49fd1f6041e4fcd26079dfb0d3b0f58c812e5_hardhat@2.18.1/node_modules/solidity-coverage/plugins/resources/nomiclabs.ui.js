const UI = require('./../../lib/ui').UI;

/**
 * Nomiclabs Plugin logging
 */
class PluginUI extends UI {
  constructor(log){
    super(log);

    this.flags = {
      testfiles:  `Path (or glob) defining a subset of tests to run`,

      testMatrix: `Generate a json object which maps which unit tests hit which lines of code.`,

      abi:        `Generate a json object which can be used to produce a unified diff of your ` +
                  `contracts public interface between two commits.`,

      solcoverjs: `Relative path from working directory to config. ` +
                  `Useful for monorepo packages that share settings.`,

      temp:       `Path to a disposable folder to store compilation artifacts in. ` +
                  `Useful when your test setup scripts include hard-coded paths to ` +
                  `a build directory.`,
    }
  }

  /**
   * Writes a formatted message via log
   * @param  {String}   kind  message selector
   * @param  {String[]} args  info to inject into template
   */
  report(kind, args=[]){
    const c = this.chalk;
    const ct = c.bold.green('>');
    const ds = c.bold.yellow('>');
    const w = ":warning:";

    const kinds = {

      'instr-skip':  `\n${c.bold('Coverage skipped for:')}` +
                     `\n${c.bold('=====================')}\n`,

      'compilation':  `\n${c.bold('Compilation:')}` +
                      `\n${c.bold('============')}\n`,

      'instr-skipped': `${ds} ${c.grey(args[0])}`,

      'versions':  `${ct} ${c.bold('ganache-core')}:      ${args[0]}\n` +
                   `${ct} ${c.bold('solidity-coverage')}: v${args[1]}`,

      'hardhat-versions': `\n${c.bold('Version')}` +
                          `\n${c.bold('=======')}\n` +
                          `${ct} ${c.bold('solidity-coverage')}: v${args[0]}`,

      'hardhat-network': `\n${c.bold('Network Info')}` +
                         `\n${c.bold('============')}\n` +
                         `${ct} ${c.bold('HardhatEVM')}: v${args[0]}\n` +
                         `${ct} ${c.bold('network')}:    ${args[1]}\n`,

      'ganache-network': `\n${c.bold('Network Info')}` +
                         `\n${c.bold('============')}\n` +
                         `${ct} ${c.bold('port')}:         ${args[1]}\n` +
                         `${ct} ${c.bold('network')}:      ${args[0]}\n`,

      'port-clash': `${w}  ${c.red("The 'port' values in your config's network url ")}` +
                          `${c.red("and .solcover.js are different. Using network's: ")} ${c.bold(args[0])}.\n`,

      'port-clash-hardhat': `${w}  ${c.red("The 'port' values in your Hardhat network's url ")}` +
                            `${c.red("and .solcover.js are different. Using Hardhat's: ")} ${c.bold(args[0])}.\n`,

    }

    this._write(kinds[kind]);
  }

  /**
   * Returns a formatted message. Useful for error message.
   * @param  {String}   kind  message selector
   * @param  {String[]} args  info to inject into template
   * @return {String}         message
   */
  generate(kind, args=[]){
    const c = this.chalk;
    const x = ":x:";

    const kinds = {
      'network-fail': `${c.red('--network argument: ')}${args[0]}` +
                      `${c.red(' is not a defined network in hardhat.config.js.')}`,

      'sources-fail': `${c.red('Cannot locate expected contract sources folder: ')} ${args[0]}`,

      'solcoverjs-fail': `${c.red('Could not load .solcover.js config file. ')}` +
                         `${c.red('This can happen if it has a syntax error or ')}` +
                         `${c.red('the path you specified for it is wrong.')}`,

      'tests-fail': `${x} ${c.bold(args[0])} ${c.red('test(s) failed under coverage.')}`,


    }


    return this._format(kinds[kind])
  }
}

module.exports = PluginUI;