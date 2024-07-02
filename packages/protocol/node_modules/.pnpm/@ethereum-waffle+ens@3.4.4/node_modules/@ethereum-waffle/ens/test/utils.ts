import {providers, Wallet} from 'ethers';
import Ganache from 'ganache-core';

export const getWallet = (): Wallet => {
  const balance = '10000000000000000000000000000000000';
  const secretKey = '0x03c909455dcef4e1e981a21ffb14c1c51214906ce19e8e7541921b758221b5ae';

  const defaultAccount = [{balance, secretKey}];

  const provider = new providers.Web3Provider(Ganache.provider({accounts: defaultAccount}) as any);
  return new Wallet(defaultAccount[0].secretKey, provider);
};
