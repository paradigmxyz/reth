module.exports = {
  roots: ["./src"],
  transform: { "^.+\\.ts?$": "ts-jest" },
  // Note: '*.test.ts' intentionally omitted; '*.spec.ts' stands out in VSCode as it is yellow
  // Also, no tsx files.
  testRegex: "(/__tests__/.*|(\\.|/)(spec))\\.ts$",
  moduleFileExtensions: ["js", "jsx", "ts"],
  globals: {
    "ts-jest": {
      tsConfig: "./tsconfig.json",
    },
  },
  testURL: "http://localhost",
  verbose: true,
};
