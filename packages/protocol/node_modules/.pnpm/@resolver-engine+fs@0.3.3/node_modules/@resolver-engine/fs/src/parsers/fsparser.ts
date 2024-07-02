import { SubParser } from "@resolver-engine/core";
import Debug from "debug";
import * as fs from "fs";
import { promisify } from "util";

const readFileAsync = promisify(fs.readFile);

const debug = Debug("resolverengine:fsparser");

export function FsParser(): SubParser<string> {
  return async (path: string): Promise<string | null> => {
    try {
      return (await readFileAsync(path, "utf-8")).toString();
    } catch (e) {
      debug(`Error returned when trying to parse "${path}", returning null`, e);
      return null;
    }
  };
}
