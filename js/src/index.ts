// Plain text from a .docx, byte-identical to mammoth's extractRawText: Rust compiled
// to WebAssembly, embedded in this module so it loads the same way in Node, browsers,
// workers and edge runtimes.
//
// Every call is synchronous CPU work on the thread that makes it (extractRawText only
// wraps it in a promise). Each runs in a fresh instance of the compiled module, so the
// memory one large document needed is released afterwards and a failed call leaves
// nothing behind.

import { WASM_BASE64 } from "../wasm/fast_docx_text.wasm.js";

/** What mammoth's extractRawText accepts, less `path` outside Node. */
export type Input =
  | { buffer: Uint8Array; arrayBuffer?: undefined; path?: undefined }
  | { arrayBuffer: ArrayBuffer; buffer?: undefined; path?: undefined }
  | { path: string; buffer?: undefined; arrayBuffer?: undefined };

export type Message = { type: "warning" | "error"; message: string };

/** mammoth's result: the text, and messages (always empty here, see the README). */
export type Result = { value: string; messages: Message[] };

type Exports = {
  memory: WebAssembly.Memory;
  alloc(len: number): number;
  extract(pointer: number, len: number): number;
  declared_xml_bytes(pointer: number, len: number): number;
  output_ptr(): number;
  output_len(): number;
};

const FUNCTIONS = [
  "alloc",
  "extract",
  "declared_xml_bytes",
  "output_ptr",
  "output_len",
] as const;

/** scripts/embed-wasm.mjs writes `r` as `-` and `R` as `_`. */
function base64Bytes(base64: string): Uint8Array<ArrayBuffer> {
  const binary = atob(base64.replaceAll("-", "r").replaceAll("_", "R"));
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

let compiled: WebAssembly.Module | null = null;

function instance(): Exports {
  compiled ??= new WebAssembly.Module(base64Bytes(WASM_BASE64));
  const { exports } = new WebAssembly.Instance(compiled, {});
  if (
    !(exports.memory instanceof WebAssembly.Memory) ||
    !FUNCTIONS.every((name) => typeof exports[name] === "function")
  ) {
    throw new Error("fast_docx_text.wasm does not export the reader's functions");
  }
  return exports as unknown as Exports;
}

/** Copies `bytes` into a fresh instance and returns it with the input's address. */
function load(bytes: Uint8Array): [Exports, number] {
  const wasm = instance();
  const pointer = wasm.alloc(bytes.byteLength);
  new Uint8Array(wasm.memory.buffer, pointer, bytes.byteLength).set(bytes);
  return [wasm, pointer];
}

const utf8 = new TextDecoder();

function bytesOf(input: Input): Uint8Array {
  if (input.buffer) return input.buffer;
  if (input.arrayBuffer) return new Uint8Array(input.arrayBuffer);
  throw new Error("Could not find file in options");
}

/** The text of a .docx, synchronously. Throws an Error with mammoth's message. */
export function extractRawTextSync(input: Input): Result {
  const bytes = bytesOf(input);
  const [wasm, pointer] = load(bytes);
  const status = wasm.extract(pointer, bytes.byteLength);
  const output = utf8.decode(
    new Uint8Array(wasm.memory.buffer, wasm.output_ptr(), wasm.output_len())
  );
  if (status !== 0) throw new Error(output);
  return { value: output, messages: [] };
}

/** A drop-in for mammoth's `extractRawText`. `path` reads the file in Node. */
export async function extractRawText(input: Input): Promise<Result> {
  if (input.path !== undefined) {
    const { readFile } = await import("node:fs/promises");
    return extractRawTextSync({ buffer: await readFile(input.path) });
  }
  return extractRawTextSync(input);
}

/**
 * What the archive declares its .xml and .rels parts inflate to, together: an upper
 * bound on the XML a read parses, since no part inflates past its declared size.
 * Reads the zip directory and the relationship parts only, so it is cheap; use it to decide whether a file is
 * worth sending to a worker thread. 0 when the archive cannot be opened.
 */
export function declaredXmlBytes(bytes: Uint8Array): number {
  const [wasm, pointer] = load(bytes);
  const size = wasm.declared_xml_bytes(pointer, bytes.byteLength);
  // The u32 comes back as a signed i32.
  return size < 0 ? size + 2 ** 32 : size;
}
