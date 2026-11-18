// The committed WebAssembly's provenance: a SHA-256 over every input of
// build-wasm.sh. `--write` records it after a build; `--check` fails when the inputs
// changed without a rebuild. Needs no Rust.
import { createHash } from "node:crypto";
import { readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";

const root = path.resolve(import.meta.dirname, "..");
const stampPath = path.join(root, "js/wasm/SOURCES.sha256");

function files(dir) {
  return readdirSync(dir).flatMap((name) => {
    const full = path.join(dir, name);
    return statSync(full).isDirectory() ? files(full) : [full];
  });
}

const inputs = [
  path.join(root, "Cargo.toml"),
  path.join(root, "Cargo.lock"),
  path.join(root, "rust-toolchain.toml"),
  ...files(path.join(root, "crates/fast-docx-text/src")),
  path.join(root, "crates/fast-docx-text/Cargo.toml"),
  path.join(root, "scripts/build-wasm.sh"),
  path.join(root, "scripts/embed-wasm.mjs"),
].sort();
const hash = createHash("sha256");
for (const file of inputs) {
  hash.update(path.relative(root, file).split(path.sep).join("/"));
  hash.update("\0");
  hash.update(readFileSync(file));
  hash.update("\0");
}
const wasm = createHash("sha256")
  .update(readFileSync(path.join(root, "js/wasm/fast_docx_text.wasm.js")))
  .digest("hex");
const stamp = `sources ${hash.digest("hex")}\nwasm ${wasm}\n`;

if (process.argv.includes("--write")) {
  writeFileSync(stampPath, stamp);
} else if (readFileSync(stampPath, "utf8") !== stamp) {
  console.error(
    "js/wasm/ is stale for its Rust sources. Run\n" +
      "  bash scripts/build-wasm.sh\n" +
      "and commit js/wasm/."
  );
  process.exit(1);
}
