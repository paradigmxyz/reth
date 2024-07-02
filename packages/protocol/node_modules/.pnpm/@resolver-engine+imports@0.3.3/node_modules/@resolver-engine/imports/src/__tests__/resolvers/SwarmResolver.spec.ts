import { resolvers } from "../..";

const data: [string, string | null][] = [
  ["", null],
  ["www.google.com", null],
  ["http://example.com", null],
  ["https://example.com", null],
  ["https://github.com/Crypto-Punkers/resolver-engine/blob/master/examples/github.ts", null],
  [
    "bzz-raw://406be87b72ce005d7f49e8fffce4e42c7b0f3da63c8218013d0a1fd994118772",
    "https://swarm-gateways.net/bzz-raw:/406be87b72ce005d7f49e8fffce4e42c7b0f3da63c8218013d0a1fd994118772",
  ],
];

describe("SwarmResolver", () => {
  const subject = resolvers.SwarmResolver();

  it.each(data)("testing %o", async (input: string, output: string | null) => {
    const actualOutput = await subject(input, { resolver: "" });
    expect(actualOutput).toBe(output);
  });
});
