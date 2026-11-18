// One read in this process; prints its peak RSS above the process after loading the
// reader and the file. Used by bench.mjs --memory.
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";

const [reader, file] = process.argv.slice(2);
const require = createRequire(path.resolve(import.meta.dirname, "../js/package.json"));
const mammoth = require("mammoth");
const { extractRawTextSync } = await import("../js/src/index.ts");
const buffer = readFileSync(file);
// Warm both readers on a tiny file so loading code is not counted as the read.
const tiny = readFileSync(require.resolve("mammoth/test/test-data/single-paragraph.docx"));
await mammoth.extractRawText({ buffer: tiny });
extractRawTextSync({ buffer: tiny });
globalThis.gc?.();
const before = process.resourceUsage().maxRSS * 1024;
let chars = 0;
try {
  chars =
    reader === "mammoth"
      ? (await mammoth.extractRawText({ buffer })).value.length
      : extractRawTextSync({ buffer }).value.length;
} catch {}
const after = process.resourceUsage().maxRSS * 1024;
console.log(JSON.stringify({ peak: after - before, chars }));
