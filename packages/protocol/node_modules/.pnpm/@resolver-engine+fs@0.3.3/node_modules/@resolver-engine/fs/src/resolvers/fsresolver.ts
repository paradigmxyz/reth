import { Context, SubResolver } from "@resolver-engine/core";
import * as fsSys from "fs";
import * as pathSys from "path";

const statAsync = (path: string): Promise<fsSys.Stats> =>
  new Promise<fsSys.Stats>((resolve, reject) => {
    fsSys.stat(path, (err, stats) => {
      if (err) {
        reject(err);
      }

      resolve(stats);
    });
  });
const NO_FILE = "ENOENT";

export function FsResolver(): SubResolver {
  return async function fs(resolvePath: string, ctx: Context): Promise<string | null> {
    const cwd = ctx.cwd || process.cwd();

    let myPath: string;
    if (!pathSys.isAbsolute(resolvePath)) {
      myPath = pathSys.join(cwd, resolvePath);
    } else {
      myPath = resolvePath;
    }
    try {
      const stats = await statAsync(myPath);
      return stats.isFile() ? myPath : null;
    } catch (e) {
      if (e.code === NO_FILE) {
        return null;
      }
      throw e;
    }
  };
}
