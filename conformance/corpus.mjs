// Every file of the public corpus (fetch-corpus.mjs) through mammoth's extractRawText
// and through fast-docx-text: the same text, or both fail.
//
//   node conformance/corpus.mjs [--mammoth <dir of a mammoth install>] [--show]
//
// The one deliberate difference: mammoth decodes a character reference to half a
// surrogate pair (&#xD800;) to a lone UTF-16 surrogate, which a Rust string cannot
// hold; fast-docx-text writes U+FFFD there, which is what the lone surrogate becomes
// as soon as the text is encoded as UTF-8. Compared after String.prototype.toWellFormed.
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";

import { extractRawTextSync } from "../js/src/index.ts";
import { localPath, lock } from "./fetch-corpus.mjs";

const at = process.argv.indexOf("--mammoth");
const mammothDir = at === -1 ? path.resolve(import.meta.dirname, "../js") : path.resolve(process.argv[at + 1]);
const require = createRequire(path.join(mammothDir, "package.json"));
const mammoth = require("mammoth");
const { version } = require("mammoth/package.json");

const counts = { same: 0, bothFail: 0, differ: 0, lone: 0 };
const differ = [];
for (const entry of lock()) {
  const buffer = readFileSync(localPath(entry));
  let want;
  try {
    const value = (await mammoth.extractRawText({ buffer })).value;
    if (!value.isWellFormed()) counts.lone++;
    want = { ok: true, text: value.toWellFormed() };
  } catch (error) {
    want = { ok: false, error: String(error) };
  }
  let got;
  try {
    got = { ok: true, text: extractRawTextSync({ buffer }).value };
  } catch (error) {
    got = { ok: false, error: String(error) };
  }
  if (want.ok && got.ok && want.text === got.text) counts.same++;
  else if (!want.ok && !got.ok) counts.bothFail++;
  else {
    counts.differ++;
    differ.push(`${entry.repo}/${entry.path}\n  mammoth: ${JSON.stringify(want).slice(0, 200)}\n  fast:    ${JSON.stringify(got).slice(0, 200)}`);
  }
}
const total = counts.same + counts.bothFail + counts.differ;
console.log(
  `mammoth ${version}: ${total} files, ${counts.same} same text, ${counts.bothFail} both fail, ${counts.differ} differ` +
    (counts.lone ? ` (${counts.lone} with lone surrogates, compared well-formed)` : "")
);
if (differ.length) {
  console.log(differ.slice(0, process.argv.includes("--show") ? 1e9 : 20).join("\n"));
  process.exit(1);
}
