import 'hardhat/types/config';
import 'hardhat/types/runtime';
import 'hardhat/types/runtime';
import {HardhatRuntimeEnvironment} from 'hardhat/types';

declare module 'hardhat/types/config' {
  type LinePreprocessor = (line: string, sourceInfo: {absolutePath: string}) => string;
  type LinePreprocessorConfig = {
    transform: LinePreprocessor;
    settings?: unknown;
    files?: string;
  };

  interface HardhatUserConfig {
    preprocess?: {
      eachLine: (
        hre: HardhatRuntimeEnvironment
      ) => LinePreprocessorConfig | Promise<LinePreprocessorConfig | undefined> | undefined;
    };
  }

  interface HardhatConfig {
    preprocess?: {
      eachLine: (
        hre: HardhatRuntimeEnvironment
      ) => LinePreprocessorConfig | Promise<LinePreprocessorConfig | undefined> | undefined;
    };
  }
}
