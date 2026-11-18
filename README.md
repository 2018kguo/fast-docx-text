<h1 align="center">fast-docx-text</h1>
<p align="center">Fast DOCX → plain text in Rust/WebAssembly. A byte-identical drop-in for mammoth's <code>extractRawText</code>.</p>
<p align="center">
  <a href="#use-it">Use it</a> ·
  <a href="#numbers">Numbers</a> ·
  <a href="#how-its-checked">How it's checked</a> ·
  <a href="#limits">Limits</a>
</p>

## Why

[mammoth](https://github.com/mwilliamson/mammoth.js) is the standard way to get the text
out of a `.docx` in JavaScript, and its `extractRawText` output is what a lot of stored
text, search indexes and chunk hashes already depend on. It is also slow on long
documents and does its parsing on the thread that calls it: a 1,000-page document holds
the event loop for well over a hundred milliseconds and hundreds of megabytes of memory.

fast-docx-text is a reimplementation of that one function, in Rust compiled to
WebAssembly, that returns **the same string, byte for byte**: on every file of a
1,610-document public corpus and on tens of thousands of generated ones, including
where mammoth fails (it fails there too). It is 11–17× faster on text-heavy documents,
uses a tenth of the memory, and has no dependencies.

## Use it

```sh
npm install @2018kguo/fast-docx-text
```

```js
import { extractRawText } from "@2018kguo/fast-docx-text";

const { value } = await extractRawText({ buffer }); // or { arrayBuffer }, or { path } in Node
```

That is mammoth's call and mammoth's result shape (`{ value, messages }`), so switching
is an import change. There is a synchronous variant too, since the work is synchronous
either way:

```js
import { extractRawTextSync } from "@2018kguo/fast-docx-text";

const { value } = extractRawTextSync({ buffer });
```

The WebAssembly is embedded in the module, so the same import works in Node 20+,
browsers, workers, Deno, Bun and edge runtimes, with no file to serve and no bundler
plugin. It needs no Rust and runs no install scripts.

**A read is synchronous CPU work** on the thread that makes it. On a server, read long
documents on a worker thread. `declaredXmlBytes(bytes)` reads only the zip directory and
the small relationship parts, and returns an upper bound on the XML a read will parse,
so it is a cheap way to decide:

```js
// docx-thread.js
import { parentPort } from "node:worker_threads";
import { extractRawTextSync } from "@2018kguo/fast-docx-text";

parentPort.on("message", ({ id, bytes }) => {
  try {
    parentPort.postMessage({ id, value: extractRawTextSync({ buffer: bytes }).value });
  } catch (error) {
    parentPort.postMessage({ id, error: String(error) });
  }
});
```

```js
// server.js
import { declaredXmlBytes, extractRawTextSync } from "@2018kguo/fast-docx-text";

// About 5 ms of work inline; past that, the thread.
const text = declaredXmlBytes(bytes) <= 1024 * 1024
  ? extractRawTextSync({ buffer: bytes }).value
  : await readOnThread(bytes); // postMessage to docx-thread.js, await its reply
```

The Rust crate is usable directly as well (`fast_docx_text::extract_raw_text(&bytes)`).

## Numbers

Measured with `npm run corpus && node bench/bench.mjs` (and `--memory`, `--size`) on an
Apple M5 Pro, Node 25, mammoth 1.13.0. The corpus is public (see
[How it's checked](#how-its-checked)); the generated documents come from
`bench/generated.mjs`, deterministically.

**Speed**, ms per document, best of 3, with the longest the event loop went without
turning in brackets:

| document | zip | text | mammoth | fast-docx-text | |
|---|--:|--:|--:|--:|--:|
| generated, 1,000 pages | 336 KB | 801 K chars | 169.5 (162) | 9.9 (10) | 17.1× |
| generated, 100 pages | 36 KB | 80 K chars | 15.3 (14) | 1.0 (1) | 14.8× |
| poi `bug65649.docx` | 464 KB | 297 K chars | 807.0 (669) | 57.1 (57) | 14.1× |
| docx4j `Docx4j_GettingStarted.docx` | 169 KB | 146 K chars | 91.9 (76) | 6.8 (7) | 13.6× |
| poi `stress010.docx` | 257 KB | 226 K chars | 96.0 (88) | 7.8 (8) | 12.3× |
| poi `stress008.docx` | 284 KB | 369 K chars | 49.0 (37) | 4.5 (4) | 11.0× |

The whole corpus, 1,610 files and 39 MB, one read each: mammoth **5.36 s** (300 files/s),
fast-docx-text **0.61 s** (2,628 files/s, 8.8×). Most corpus files are tiny test
documents, where a fixed per-call cost dominates (a one-paragraph file: 0.13 ms against
mammoth's 1.0 ms).

These are warm numbers. The first read in a process also compiles the module, about
10 ms on this machine, and runs before the engine has optimized it: the first read of
`bug65649.docx` takes about 75 ms.

**Memory**, peak RSS of one read in a fresh process, above the process before it:

| document | text | mammoth | fast-docx-text |
|---|--:|--:|--:|
| generated, 5,000 pages | 4.02 M chars | 832 MB | 62 MB |
| generated, 1,000 pages | 0.80 M chars | 249 MB | 23 MB |
| poi `bug65649.docx` | 0.30 M chars | 721 MB | 63 MB |
| poi `stress010.docx` | 0.23 M chars | 154 MB | 18 MB |
| poi `stress008.docx` | 0.37 M chars | 84 MB | 16 MB |
| one 120 MB paragraph (near the 128 MiB part cap) | 120 M chars | 373 MB | 367 MB |

For ordinary documents a read peaks at about four times the XML it parses (the XML,
its parsed tree and the text); a document that is one huge run of text costs about
three copies of it, as it does in mammoth. Reads share one instance of the module
while its memory stays under 64 MiB. WebAssembly memory never shrinks, so a read that
grows it past that drops the instance and the memory with it; a process never holds
more than 64 MiB between reads.

**Size**:

| | raw | gzip | brotli |
|---|--:|--:|--:|
| WebAssembly | 235 KB | 96 KB | 79 KB |
| as shipped (base64 in a JS module) | 314 KB | 129 KB | 106 KB |
| JS glue | 4 KB | 2 KB | 1 KB |

The module is embedded as base64 so it loads the same way in every runtime and the
synchronous API works without a file to fetch; that costs a third more raw and about
a quarter more compressed than a separate `.wasm` file. It is built at `opt-level = 3`:
`s` is 6% smaller and 18% slower, `z` 10% smaller and 75% slower, and `wasm-opt` saves
nothing once compressed.

Installed: 340 KB in one package with no dependencies. mammoth installs 7.6 MB across 24
packages (though it also converts to HTML and Markdown, which this does not).

## How it's checked

- **The public corpus.** `conformance/fetch-corpus.mjs` fetches every `.docx` in four
  permissively licensed projects at pinned commits, each checked against its SHA-256 in
  `conformance/corpus.lock.json`: mammoth.js's test data (BSD-2-Clause), python-docx
  (MIT), docx4j (Apache-2.0) and Apache POI (Apache-2.0), 1,610 files.
  `conformance/corpus.mjs` reads each with both: **1,522 give the same text and 88 fail
  in both; none differ.**
- **Generated documents.** Corpora barely exercise tracked moves, deleted paragraph
  marks and rows, fields, checkboxes, comments, equations, text boxes, entities, CDATA,
  CRLF, byte order marks, other namespace prefixes, numbering style links or malformed
  XML, so `conformance/differential.mjs` generates documents that mix them from a seed and
  compares live: **20,000 seeds, none differ** (2,000 in CI on every push).
- **Against the latest mammoth.** CI runs both against the pinned mammoth and against
  whatever `npm install mammoth@latest` gives, so a new mammoth release that changes
  output shows up as a failing build.
- **Fuzzing.** `crates/fast-docx-text/tests/mutations.rs` damages valid packages at
  random (bit flips, truncation, splices, broken sizes and offsets) and requires every
  read to end in text or an error, never a panic; `fuzz/` is a cargo-fuzz target for
  coverage-guided runs, which CI also runs for a few minutes.
- **The WebAssembly is the build of its sources.** It is committed (so the package needs
  no Rust), built reproducibly in a pinned Rust image, and CI rebuilds it and compares
  byte for byte; `scripts/stamp.mjs --check` verifies the recorded source hash.

### The one deliberate difference

mammoth decodes a character reference to half of a surrogate pair (`&#xD800;`) to a lone
UTF-16 surrogate, which a Rust string cannot hold. fast-docx-text writes U+FFFD there,
which is what that lone surrogate becomes as soon as the text is encoded as UTF-8
(written to a file, sent over HTTP, stored in most databases). The conformance checks
compare after `String.prototype.toWellFormed()`.

`messages` is always empty: mammoth's warnings about unrecognised elements are not
reproduced. Error messages are mammoth's where that was simple, and otherwise close; a
file fails in one exactly when it fails in the other.

## Limits

Every input is treated as untrusted:

- **Inflate.** A part inflates to at most 128 MiB, and never past the uncompressed size
  the archive declares for it: a part that would inflate further fails with mammoth's own
  error (`uncompressed data size mismatch`). So `declaredXmlBytes` is a true upper bound,
  and a zip bomb costs at most what it declares.
- **Memory.** The module's memory is capped at 1 GiB; a document that needs more fails
  with an error instead of growing the process. A call that traps (out of memory)
  drops its instance, so the next call starts clean.
- **Nesting.** XML nests at most 4,096 levels (mammoth's own recursion gives out
  somewhere past 1,800, so every document mammoth reads, this reads); deeper fails
  cleanly. The WebAssembly stack is 8 MiB.
- **Errors, not crashes.** Sizes and offsets from the file are checked with saturating
  arithmetic; nothing in the reader panics on malformed input.

## Working on it

```sh
npm install
npm run corpus                         # fetch the public corpus (42 MB)
bash scripts/build-wasm.sh --local     # build the WebAssembly with your toolchain
npm test                               # Rust tests and the package tests
npm run conformance                    # the corpus and 2,000 generated documents
node conformance/differential.mjs --cases 20000
bash scripts/build-wasm.sh             # the reproducible build (Docker off x86_64 Linux)
```

## License

MIT, except the parts that follow mammoth and dingbat-to-unicode (Copyright (c) 2013,
Michael Williamson), which are BSD-2-Clause; see [NOTICE](NOTICE).
