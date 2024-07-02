import 'hardhat/types/config';

declare module 'hardhat/types/config' {
  interface HardhatUserConfig {
    docgen?: {
      path?: string,
      clear?: boolean,
      runOnCompile?: boolean,
      only?: string[],
      except?: string[],
    }
  }

  interface HardhatConfig {
    docgen: {
      path: string,
      clear: boolean,
      runOnCompile: boolean,
      only: string[],
      except: string[],
    }
  }
}
