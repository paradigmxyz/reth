export type ContractAddressOrInstance = string | { getAddress(): Promise<string> };

export async function getContractAddress(addressOrInstance: ContractAddressOrInstance): Promise<string> {
  if (typeof addressOrInstance === 'string') {
    return addressOrInstance;
  } else {
    return await addressOrInstance.getAddress();
  }
}
