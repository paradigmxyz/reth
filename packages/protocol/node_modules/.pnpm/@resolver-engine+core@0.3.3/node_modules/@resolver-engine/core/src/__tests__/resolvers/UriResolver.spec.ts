import { resolvers } from "../..";

const data: [string, string | null][] = [
  ["", null],
  ["www.google.com", null],
  ["http://example.com", "http://example.com/"],
  ["https://example.com/", "https://example.com/"],
  [
    "https://github.com/Crypto-Punkers/resolver-engine/blob/master/examples/github.ts",
    "https://github.com/Crypto-Punkers/resolver-engine/blob/master/examples/github.ts",
  ],
  [ "c:/foobar/file.sol", null ],
  [ "c:\\foobar\\file.sol", null ]
];

describe("UriResolver", () => {
  const subject = resolvers.UriResolver();

  it.each(data)("testing %o", async (input: string, output: string | null) => {
    const actualOutput = await subject(input, { resolver: "" });
    expect(actualOutput).toBe(output);
  });
});
