import { resolvers } from "../..";

const data: [string, string | null][] = [
  ["", null],
  [
    "https://github.com/Crypto-Punkers/resolver-engine/blob/master/examples/github.ts",
    "https://raw.githubusercontent.com/Crypto-Punkers/resolver-engine/master/examples/github.ts",
  ],
  [
    "https://github.com/Crypto-Punkers/resolver-engine/blob/cymerrad/remix-ide-integration/examples/github.ts",
    "https://raw.githubusercontent.com/Crypto-Punkers/resolver-engine/cymerrad/remix-ide-integration/examples/github.ts",
  ],
  [
    "github:Crypto-Punkers/resolver-engine/examples/github.ts",
    "https://raw.githubusercontent.com/Crypto-Punkers/resolver-engine/master/examples%2Fgithub.ts",
  ],
  [
    "https://github.com/Crypto-Punkers/resolver-engine/examples/github.ts",
    "https://raw.githubusercontent.com/Crypto-Punkers/resolver-engine/master/examples/github.ts",
  ],
  [
    "github:Crypto-Punkers/resolver-engine/examples/github.ts#development",
    "https://raw.githubusercontent.com/Crypto-Punkers/resolver-engine/development/examples%2Fgithub.ts",
  ],
  [
    "github:Crypto-Punkers/resolver-engine/examples/github.ts#92f6fd0",
    "https://raw.githubusercontent.com/Crypto-Punkers/resolver-engine/92f6fd0/examples%2Fgithub.ts",
  ],
  [
    "bitbucket:Crypto-Punkers/resolver-engine/examples/github.ts",
    "https://bitbucket.org/Crypto-Punkers/resolver-engine/raw/master/examples%2Fgithub.ts",
  ],
  [
    "gitlab:eql/EQL5-Android/examples/REPL/make.lisp",
    "https://gitlab.com/eql/EQL5-Android/raw/master/examples%2FREPL%2Fmake.lisp",
  ],
  [
    "bitbucket:Crypto-Punkers/resolver-engine/examples/github.ts#development",
    "https://bitbucket.org/Crypto-Punkers/resolver-engine/raw/development/examples%2Fgithub.ts",
  ],
  [
    "gitlab:eql/EQL5-Android/examples/REPL/make.lisp#270ab92c0f55c30d35ad9317399770926ec4adac",
    "https://gitlab.com/eql/EQL5-Android/raw/270ab92c0f55c30d35ad9317399770926ec4adac/examples%2FREPL%2Fmake.lisp",
  ],
  [
    "github:Crypto-Punkers/resolver-engine/examples/github.ts#cymerrad/remix-ide-integration",
    "https://raw.githubusercontent.com/Crypto-Punkers/resolver-engine/cymerrad%2Fremix-ide-integration/examples%2Fgithub.ts",
  ],
];

describe("GitHubResolver", () => {
  const subject = resolvers.GithubResolver();

  it.each(data)("testing %o", async (input: string, output: string | null) => {
    const actualOutput = await subject(input, { resolver: "" });
    expect(actualOutput).toBe(output);
  });
});
