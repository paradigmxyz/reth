import { resolvers } from "../..";

const data: [string, string | null][] = [
  ["", null],
  ["www.google.com", null],
  ["http://example.com", null],
  ["https://example.com", null],
  ["https://github.com/Crypto-Punkers/resolver-engine/blob/master/examples/github.ts", null],
  ["ipfs://123/path/to/resource", "https://gateway.ipfs.io/ipfs/123/path/to/resource"],
];

describe("IPFSResolver", () => {
  const subject = resolvers.IPFSResolver();

  it.each(data)("testing %o", async (input: string, output: string | null) => {
    const actualOutput = await subject(input, { resolver: "" });
    expect(actualOutput).toBe(output);
  });
});
