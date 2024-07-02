import "hardhat/types/config";

declare module "hardhat/types/config" {
  interface HardhatUserConfig {
    contractSizer?: {
      alphaSort?: boolean;
      disambiguatePaths?: boolean;
      runOnCompile?: boolean;
      strict?: boolean;
      only?: string[];
      except?: string[];
      outputFile?: string;
      unit?: 'B' | 'kB' | 'KiB';
    };
  }

  interface HardhatConfig {
    contractSizer: {
      alphaSort: boolean;
      disambiguatePaths: boolean;
      runOnCompile: boolean;
      strict: boolean;
      only: string[];
      except: string[];
      outputFile: string;
      unit: 'B' | 'kB' | 'KiB';
    };
  }
}
