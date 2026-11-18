// fast-docx-text against mammoth on the public corpus (conformance/fetch-corpus.mjs):
//
//   node bench/bench.mjs                 the largest files and the whole corpus
//   node bench/bench.mjs --top 8 --runs 5
//   node bench/bench.mjs --memory        peak RSS per read, each in a fresh process
//   node bench/bench.mjs --size          package and install size
//
// Times are wall milliseconds, best of --runs, single-threaded. "Longest stall" is the
// longest the event loop went without turning during a read: fast-docx-text is one
// synchronous call, so it stalls for its whole time (run it on a worker for large
// files); mammoth is asynchronous but does its parsing in long synchronous stretches.
import { execFileSync } from "node:child_process";
import { readFileSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import os from "node:os";
import path from "node:path";
import { monitorEventLoopDelay, performance } from "node:perf_hooks";

import { localPath, lock } from "../conformance/fetch-corpus.mjs";

const root = path.resolve(import.meta.dirname, "..");
const require = createRequire(path.join(root, "js/package.json"));
const flag = (name, fallback) => {
  const at = process.argv.indexOf(name);
  return at === -1 ? fallback : Number(process.argv[at + 1]);
};

const mammoth = require("mammoth");
const { extractRawTextSync } = await import("../js/src/index.ts");
const readers = {
  mammoth: async (buffer) => (await mammoth.extractRawText({ buffer })).value,
  "fast-docx-text": (buffer) => extractRawTextSync({ buffer }).value,
};
const files = lock().map((entry) => ({ entry, file: localPath(entry), size: statSync(localPath(entry)).size }));

console.log(
  `${os.cpus()[0].model}, ${os.platform()} ${os.arch()}, Node ${process.version}, ` +
    `mammoth ${require("mammoth/package.json").version}, fast-docx-text ${require("./package.json").version}\n`
);

async function time(read, buffer, runs) {
  let best = { ms: Infinity, stall: 0 };
  for (let run = 0; run < runs; run++) {
    const delay = monitorEventLoopDelay({ resolution: 1 });
    const tick = setInterval(() => {}, 1);
    delay.enable();
    const start = performance.now();
    let last = start;
    let stall = 0;
    // A probe that measures the longest gap between turns of the loop.
    const probe = setInterval(() => {
      const now = performance.now();
      stall = Math.max(stall, now - last);
      last = now;
    }, 0);
    try {
      await read(buffer);
    } catch {
      // A file a reader refuses costs what it cost.
    }
    const ms = performance.now() - start;
    stall = Math.max(stall, performance.now() - last);
    clearInterval(probe);
    clearInterval(tick);
    delay.disable();
    if (ms < best.ms) best = { ms, stall };
  }
  return best;
}

if (process.argv.includes("--memory")) {
  // One read per fresh process: peak RSS above an idle process that loaded the same reader.
  const { writeFileSync, mkdtempSync } = await import("node:fs");
  const { generatedDocx } = await import("./generated.mjs");
  const textOf = (file) => {
    try {
      return extractRawTextSync({ buffer: readFileSync(file) }).value.length;
    } catch {
      return 0;
    }
  };
  const top = files
    .map((row) => ({ ...row, chars: textOf(row.file) }))
    .toSorted((a, b) => b.chars - a.chars)
    .slice(0, flag("--top", 3));
  const dir = mkdtempSync(path.join(os.tmpdir(), "fast-docx-text-"));
  for (const pages of [1000, 5000]) {
    const file = path.join(dir, `generated-${pages}-pages.docx`);
    writeFileSync(file, generatedDocx(pages));
    top.push({ entry: { path: file }, file, size: statSync(file).size });
  }
  const child = path.join(import.meta.dirname, "memory-child.mjs");
  console.log("| file | zip | text | mammoth peak RSS | fast-docx-text peak RSS |\n|---|--:|--:|--:|--:|");
  for (const { entry, file, size } of top) {
    const peak = (reader) =>
      JSON.parse(execFileSync(process.execPath, [child, reader, file], { encoding: "utf8" }));
    const m = peak("mammoth");
    const f = peak("fast-docx-text");
    console.log(
      `| ${path.basename(entry.path)} | ${(size / 1024).toFixed(0)} KB | ${(f.chars / 1e6).toFixed(2)} M chars | ${(m.peak / 2 ** 20).toFixed(0)} MB | ${(f.peak / 2 ** 20).toFixed(0)} MB |`
    );
  }
} else if (process.argv.includes("--size")) {
  const { gzipSync, brotliCompressSync } = await import("node:zlib");
  const wasmJs = readFileSync(path.join(root, "js/wasm/fast_docx_text.wasm.js"));
  const encoded = /"([A-Za-z0-9+/=_-]+)"/.exec(wasmJs.toString())[1];
  const wasm = Buffer.from(encoded.replaceAll("-", "r").replaceAll("_", "R"), "base64");
  const glue = readFileSync(path.join(root, "js/dist/src/index.js"));
  const row = (name, bytes) =>
    console.log(
      `| ${name} | ${(bytes.length / 1024).toFixed(0)} KB | ${(gzipSync(bytes, { level: 9 }).length / 1024).toFixed(0)} KB | ${(brotliCompressSync(bytes).length / 1024).toFixed(0)} KB |`
    );
  console.log("| | raw | gzip | brotli |\n|---|--:|--:|--:|");
  row("WebAssembly", wasm);
  row("WebAssembly as base64 JS (what ships)", wasmJs);
  row("JS glue", glue);
} else {
  const runs = flag("--runs", 3);
  const { generatedDocx } = await import("./generated.mjs");
  const textOf = (buffer) => {
    try {
      return extractRawTextSync({ buffer }).value.length;
    } catch {
      return 0;
    }
  };
  // The corpus files with the most text, then generated long documents.
  const rows = files
    .map(({ entry, file, size }) => ({ name: path.basename(entry.path), buffer: readFileSync(file), size }))
    .map((row) => ({ ...row, chars: textOf(row.buffer) }))
    .toSorted((a, b) => b.chars - a.chars)
    .slice(0, flag("--top", 4));
  for (const pages of [100, 1000]) {
    const buffer = Buffer.from(generatedDocx(pages));
    rows.push({ name: `generated, ${pages} pages`, buffer, size: buffer.length, chars: textOf(buffer) });
  }
  console.log("Documents with the most text, ms (longest event-loop stall):\n");
  console.log(`| file | zip | text | ${Object.keys(readers).join(" | ")} | speedup |\n|---|--:|--:|--:|--:|--:|`);
  for (const { name, buffer, size, chars } of rows) {
    const results = [];
    for (const read of Object.values(readers)) results.push(await time(read, buffer, runs));
    console.log(
      `| ${name} | ${(size / 1024).toFixed(0)} KB | ${(chars / 1000).toFixed(0)} K chars | ` +
        results.map((r) => `${r.ms.toFixed(1)} (${r.stall.toFixed(0)})`).join(" | ") +
        ` | ${(results[0].ms / results[1].ms).toFixed(1)}× |`
    );
  }
  const totals = Object.fromEntries(Object.keys(readers).map((name) => [name, 0]));
  let bytes = 0;
  for (const { file, size } of files) {
    const buffer = readFileSync(file);
    bytes += size;
    for (const [name, read] of Object.entries(readers)) {
      const start = performance.now();
      try {
        await read(buffer);
      } catch {}
      totals[name] += performance.now() - start;
    }
  }
  console.log(`\nWhole corpus (${files.length} files, ${(bytes / 2 ** 20).toFixed(0)} MB, one run each):\n`);
  console.log("| | total | files/s | MB/s |\n|---|--:|--:|--:|");
  for (const [name, ms] of Object.entries(totals)) {
    console.log(
      `| ${name} | ${(ms / 1000).toFixed(2)} s | ${(files.length / (ms / 1000)).toFixed(0)} | ${(bytes / 2 ** 20 / (ms / 1000)).toFixed(1)} |`
    );
  }
}
