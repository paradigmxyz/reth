declare module "ganache-core" {
  import { Server as HttpServer } from "http";
  export interface JsonRpcPayload {
    jsonrpc: string;
    method: string;
    params: any[];
    id?: string | number;
  }

  export interface JsonRpcResponse {
    jsonrpc: string;
    id: number;
    result?: any;
    error?: string;
  }

  namespace Ganache {
    export interface IProviderOptions {
      account_keys_path?: string;
      accounts?: object[];
      allowUnlimitedContractSize?: boolean;
      blockTime?: number;
      db_path?: string;
      debug?: boolean;
      default_balance_ether?: number;
      fork?: string | object;
      fork_block_number?: string | number;
      forkCacheSize?: number;
      gasLimit?: string | number;
      gasPrice?: string;
      hardfork?: "byzantium" | "constantinople" | "petersburg" | "istanbul" | "muirGlacier";
      hd_path?: string;
      locked?: boolean;
      logger?: {
        log(msg: string): void;
      };
      mnemonic?: string;
      network_id?: number;
      networkId?: number;
      port?: number;
      seed?: any;
      time?: Date;
      total_accounts?: number;
      unlocked_accounts?: string[];
      verbose?: boolean;
      vmErrorsOnRPCResponse?: boolean;
      ws?: boolean;
    }

    export interface IServerOptions extends IProviderOptions {
      keepAliveTimeout?: number;
    }

    export function provider(opts?: IProviderOptions): Provider;
    export function server(opts?: IServerOptions): Server;

    export interface Server extends HttpServer {
      provider: Provider
    }

    export interface Provider {
      send(
          payload: JsonRpcPayload,
          callback: (error: Error | null, result?: JsonRpcResponse) => void
      ): void;
  
      on(type: string, callback: () => void): void;
  
      once(type: string, callback: () => void): void;
  
      removeListener(type: string, callback: () => void): void;
  
      removeAllListeners(type: string): void;

      close: (callback: Function) => void;
    }
  }
  export default Ganache;
}
