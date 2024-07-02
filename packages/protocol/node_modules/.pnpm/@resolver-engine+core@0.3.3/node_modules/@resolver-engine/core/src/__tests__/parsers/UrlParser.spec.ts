import nock from "nock";
import { parsers } from "../..";

const incorrectUrls = [["", null], ["www.google.com", null], [ "c:/foobar/file.sol", null ], [ "c:\\foobar\\file.sol", null ]];
const correctUrls = [["http://example.com/some/file.txt", "http://example.com/some/file.txt"]];
const data: [string, string | null][] = incorrectUrls.concat(correctUrls) as any;

describe("UrlParser", () => {
  const parser = parsers.UrlParser();

  beforeAll(() => {
    nock.disableNetConnect();
    correctUrls.forEach(pair => {
      const [url, content] = pair;

      if (content) {
        nock(url)
          .get("")
          .reply(200, content)
          .persist();
      } else {
        nock(url)
          .get("")
          .reply(404, "")
          .persist();
      }
    });
  });

  it.each(data)("should parse %o into %o", async (input: string, expected: string | null) => {
    const output = await parser(input, { resolver: "doesn't matter" });
    expect(output).toBe(expected);
  });
});
