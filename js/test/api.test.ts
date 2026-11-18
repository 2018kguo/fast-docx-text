import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { test } from "node:test";

import { strToU8, zipSync } from "fflate";
import mammoth from "mammoth";

import { declaredXmlBytes, extractRawText, extractRawTextSync } from "../src/index.ts";

const require = createRequire(import.meta.url);
const fixture = (name: string) =>
  readFileSync(require.resolve(`mammoth/test/test-data/${name}`));

/** A minimal package around `body`. */
function docx(body: string): Uint8Array {
  return zipSync({
    "[Content_Types].xml": strToU8(
      '<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/></Types>'
    ),
    "_rels/.rels": strToU8(
      '<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>'
    ),
    "word/document.xml": strToU8(
      `<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>${body}</w:body></w:document>`
    ),
  });
}

test("the same result as mammoth, from a buffer, an ArrayBuffer or a path", async () => {
  for (const name of ["single-paragraph.docx", "tables.docx", "footnotes.docx", "text-box.docx"]) {
    const buffer = fixture(name);
    const want = await mammoth.extractRawText({ buffer });
    assert.deepEqual(await extractRawText({ buffer }), { value: want.value, messages: [] });
    const arrayBuffer = buffer.buffer.slice(buffer.byteOffset, buffer.byteOffset + buffer.byteLength);
    assert.equal(extractRawTextSync({ arrayBuffer }).value, want.value);
    assert.equal(
      (await extractRawText({ path: require.resolve(`mammoth/test/test-data/${name}`) })).value,
      want.value
    );
  }
});

test("a file mammoth refuses is refused, as an Error", async () => {
  await assert.rejects(mammoth.extractRawText({ buffer: Buffer.from("not a zip") }));
  await assert.rejects(extractRawText({ buffer: new TextEncoder().encode("not a zip") }), Error);
  assert.throws(() => extractRawTextSync({ buffer: new Uint8Array() }), Error);
});

test("a view into a larger buffer reads only its own bytes", () => {
  const bytes = fixture("single-paragraph.docx");
  const padded = new Uint8Array(bytes.byteLength + 200);
  padded.set(bytes, 100);
  assert.equal(
    extractRawTextSync({ buffer: padded.subarray(100, 100 + bytes.byteLength) }).value,
    "Walking on imported air\n\n"
  );
});

test("declaredXmlBytes sees a deflate bomb in a few KiB", () => {
  const bomb = docx(`<w:p><w:r><w:t>${" ".repeat(8 * 1024 * 1024)}</w:t></w:r></w:p>`);
  assert.ok(bomb.byteLength < 64 * 1024);
  assert.ok(declaredXmlBytes(bomb) > 8 * 1024 * 1024);
  assert.equal(declaredXmlBytes(new Uint8Array(10)), 0);
});

test("declaredXmlBytes counts a main part not named .xml", async () => {
  // A read follows the officeDocument relationship to any target.
  const document = strToU8(
    `<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>${"x".repeat(2_000_000)}</w:t></w:r></w:p></w:body></w:document>`
  );
  const bytes = zipSync({
    "_rels/.rels": strToU8(
      '<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.dat"/></Relationships>'
    ),
    "word/document.dat": document,
  });
  const want = await mammoth.extractRawText({ buffer: Buffer.from(bytes) });
  assert.equal(extractRawTextSync({ buffer: bytes }).value, want.value);
  assert.ok(declaredXmlBytes(bytes) >= document.length);

  // Relationship parts past 16 KiB are not parsed by the size probe: every part
  // counts instead.
  const padded = zipSync({
    "_rels/.rels": strToU8(
      `<?xml version="1.0"?><!--${" ".repeat(20_000)}--><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.dat"/></Relationships>`
    ),
    "word/document.dat": document,
  });
  assert.ok(declaredXmlBytes(padded) >= document.length + 20_000);
});

test("a part never inflates past its declared size", async () => {
  const bytes = docx(`<w:p><w:r><w:t>${"x".repeat(1_000_000)}</w:t></w:r></w:p>`);
  const view = new DataView(bytes.buffer);
  for (let at = 0; at + 4 <= bytes.length; at += 1) {
    const signature = view.getUint32(at, true);
    // Local header (size at +22) and central directory entry (size at +24).
    const size = signature === 0x04034b50 ? at + 22 : signature === 0x02014b50 ? at + 24 : -1;
    if (size !== -1 && view.getUint32(size, true) > 1_000_000) {
      view.setUint32(size, 1000, true);
    }
  }
  await assert.rejects(mammoth.extractRawText({ buffer: Buffer.from(bytes) }));
  assert.throws(() => extractRawTextSync({ buffer: bytes }), /size mismatch/);
});

test("a size near 4 GiB in the central directory fails as an error, not a trap", () => {
  const bytes = docx("<w:p><w:r><w:t>x</w:t></w:r></w:p>");
  const view = new DataView(bytes.buffer);
  let central = bytes.length - 4;
  while (view.getUint32(central, true) !== 0x02014b50) central -= 1;
  view.setUint32(central + 20, 0xfffffff0, true);
  assert.throws(
    () => extractRawTextSync({ buffer: bytes }),
    (error) => error instanceof Error && error.constructor === Error
  );
});

test("nesting mammoth reads, this reads too", async () => {
  // 1,800 levels: mammoth's own recursion gives out somewhere past that.
  const bytes = docx(
    `${"<w:sdt><w:sdtContent>".repeat(900)}<w:p><w:r><w:t>x</w:t></w:r></w:p>${"</w:sdtContent></w:sdt>".repeat(900)}`
  );
  const want = await mammoth.extractRawText({ buffer: Buffer.from(bytes) });
  assert.equal(extractRawTextSync({ buffer: bytes }).value, want.value);
  // Past the cap: a clean error.
  const deep = docx(`${"<w:sdt><w:sdtContent>".repeat(2100)}${"</w:sdtContent></w:sdt>".repeat(2100)}`);
  assert.throws(() => extractRawTextSync({ buffer: deep }), /depth cap/);
});

test("the module imports nothing and caps its memory", async () => {
  const { WASM_BASE64 } = await import("../wasm/fast_docx_text.wasm.js");
  const base64 = WASM_BASE64.replaceAll("-", "r").replaceAll("_", "R");
  const module = new WebAssembly.Module(Buffer.from(base64, "base64"));
  assert.deepEqual(WebAssembly.Module.imports(module), []);
  const memory = new WebAssembly.Instance(module, {}).exports.memory as WebAssembly.Memory;
  assert.throws(() => memory.grow(65536 - memory.buffer.byteLength / 65536 + 1));
});

test("reads in a row, across an instance dropped for its size, match reads alone", async () => {
  // About 80 MB of text grows the shared instance past what it keeps.
  const large = docx(`<w:p><w:r><w:t>${"x".repeat(80_000_000)}</w:t></w:r></w:p>`);
  const small = fixture("tables.docx");
  const want = (await mammoth.extractRawText({ buffer: small })).value;
  assert.equal(extractRawTextSync({ buffer: small }).value, want);
  assert.equal(extractRawTextSync({ buffer: large }).value.length, 80_000_002);
  assert.throws(() => extractRawTextSync({ buffer: new Uint8Array(10) }), Error);
  assert.equal(extractRawTextSync({ buffer: small }).value, want);
});

test("every dingbat reads as mammoth reads it", async () => {
  const fonts = ["Symbol", "Webdings", "Wingdings", "wingdings 2", "WINGDINGS 3", "Arial"];
  const chars = [];
  for (let code = 0; code < 256; code++) {
    const hex = code.toString(16).toUpperCase().padStart(2, "0");
    chars.push(hex, `F0${hex}`);
  }
  chars.push("", "-1", "100", "F100", "1e2", " 41", "0x41", "zz", "41.5");
  const body = fonts
    .map((font) =>
      chars.map((char) => `<w:p><w:r><w:sym w:font="${font}" w:char="${char}"/></w:r></w:p>`).join("")
    )
    .join("");
  const bytes = docx(body);
  const want = await mammoth.extractRawText({ buffer: Buffer.from(bytes) });
  assert.equal(extractRawTextSync({ buffer: bytes }).value, want.value);
});
