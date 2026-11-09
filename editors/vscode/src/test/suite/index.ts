// Mocha runner loaded inside the editor by @vscode/test-electron. It runs
// every *.test.js beside it.

import * as path from "path";
import { readdirSync } from "fs";
import Mocha from "mocha";

export function run(): Promise<void> {
  const mocha = new Mocha({ ui: "bdd", color: true, timeout: 60000 });
  const here = __dirname;
  for (const file of readdirSync(here)) {
    if (file.endsWith(".test.js")) {
      mocha.addFile(path.join(here, file));
    }
  }
  return new Promise((resolve, reject) => {
    mocha.run((failures) => {
      if (failures > 0) {
        reject(new Error(`${failures} test(s) failed`));
      } else {
        resolve();
      }
    });
  });
}
