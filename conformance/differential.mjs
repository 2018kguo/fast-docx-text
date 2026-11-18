// fast-docx-text against mammoth, live, on generated documents. Public corpora
// barely exercise moves, deleted paragraph marks and rows, fields, checkboxes,
// comments, equations, text boxes, entities, CDATA, XML comments inside text, CRLF,
// byte order marks, other namespace prefixes, numbering style links and malformed XML,
// so each case here mixes them at random from a fixed seed. Both must give the same
// text, or both must fail. The one deliberate difference (a lone surrogate from
// &#xD800; becomes U+FFFD) is compared after String.prototype.toWellFormed.
//
//   node conformance/differential.mjs [--cases 20000] [--seed 1] [--mammoth <dir>]

import { createRequire } from "node:module";
import path from "node:path";

import { strToU8, zipSync } from "fflate";

import { extractRawTextSync } from "../js/src/index.ts";

const flag = (name, fallback) => {
  const at = process.argv.indexOf(name);
  return at === -1 ? fallback : process.argv[at + 1];
};
const CASES = Number(flag("--cases", 2000));
const FIRST = Number(flag("--seed", 1));
const mammothDir = path.resolve(flag("--mammoth", path.join(import.meta.dirname, "../js")));
const require = createRequire(path.join(mammothDir, "package.json"));
const mammoth = require("mammoth");
const { version } = require("mammoth/package.json");

function rng(seed) {
  let state = seed >>> 0;
  const next = () => {
    state = (state + 0x6d_2b_79_f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4_294_967_296;
  };
  return {
    chance: (p) => next() < p,
    int: (n) => Math.floor(next() * n),
    pick: (items) => {
      const item = items[Math.floor(next() * items.length)];
      if (item === undefined) {
        throw new Error("pick from an empty list");
      }
      return item;
    },
  };
}

const WORDS = [
  "Section",
  "the Buyer",
  " shall ",
  "indemnify",
  "café",
  "合同",
  "“quoted”",
  "a b",
  "  spaced  ",
  "",
  "1.5",
  "0.18",
  "emoji 😀",
  "&amp;",
  "&lt;tag&gt;",
  "&#x41;&#66;",
  "&quot;q&apos;",
  "tab\there",
];

const AUTHORS = ["Counsel", 'Opposing "Party"', "", "Ann & Co"];

function words(r) {
  const parts = Array.from({ length: 1 + r.int(3) }, () => r.pick(WORDS));
  return parts.join(r.pick([" ", "", "-"]));
}

function author(r) {
  const name = r
    .pick(AUTHORS)
    .replaceAll("&", "&amp;")
    .replaceAll('"', "&quot;");
  return r.chance(0.8) ? ` ${"w"}:author="${name}"` : "";
}

function runContent(r, o, deleted) {
  const w = o.w;
  const items = [];
  for (let i = 0, n = 1 + r.int(3); i < n; i += 1) {
    const kind = r.int(30);
    if (kind < 12) {
      const tag = deleted && r.chance(0.8) ? "delText" : "t";
      const space = r.chance(0.5) ? ' xml:space="preserve"' : "";
      items.push(`<${w}:${tag}${space}>${words(r)}</${w}:${tag}>`);
    } else if (kind === 12) {
      items.push(`<${w}:tab/>`);
    } else if (kind === 13) {
      items.push(
        r.pick([`<${w}:br/>`, `<${w}:br ${w}:type="page"/>`, `<${w}:cr/>`])
      );
    } else if (kind === 14) {
      items.push(r.pick([`<${w}:noBreakHyphen/>`, `<${w}:softHyphen/>`]));
    } else if (kind === 15) {
      items.push(
        r.pick([
          `<${w}:sym ${w}:font="Wingdings" ${w}:char="F0FC"/>`,
          `<${w}:sym ${w}:font="Symbol" ${w}:char="F0B7"/>`,
          `<${w}:sym ${w}:font="Symbol" ${w}:char="61"/>`,
          `<${w}:sym ${w}:font="Arial" ${w}:char="F0FC"/>`,
        ])
      );
    } else if (kind === 16 && o.comments.length > 0) {
      items.push(`<${w}:commentReference ${w}:id="${r.pick(o.comments)}"/>`);
    } else if (kind === 17 && r.chance(0.15)) {
      items.push(`<${w}:t>a<!-- note -->b</${w}:t>`);
    } else if (kind === 18 && r.chance(0.15)) {
      items.push(`<${w}:t><![CDATA[cdata & <raw>]]></${w}:t>`);
    } else if (kind === 19) {
      items.push(`<${w}:lastRenderedPageBreak/>`);
    } else if (kind === 20) {
      items.push(`<${w}:footnoteReference ${w}:id="1"/>`);
    } else if (kind === 21) {
      items.push(
        `<${w}:instrText xml:space="preserve"> PAGE </${w}:instrText>`
      );
    } else {
      items.push(`<${w}:t>${words(r)}</${w}:t>`);
    }
  }
  return items.join("");
}

function run(r, o, deleted = false) {
  const w = o.w;
  const properties = r.chance(0.3) ? `<${w}:rPr><${w}:b/></${w}:rPr>` : "";
  return `<${w}:r>${properties}${runContent(r, o, deleted)}</${w}:r>`;
}

/** A complete field: begin, code, separate, result, end (sometimes tracked). */
function field(r, o) {
  const w = o.w;
  const code = r.pick([
    " PAGE ",
    ' HYPERLINK "https://x.test" ',
    " FORMCHECKBOX ",
    ' XE "x" ',
  ]);
  const marker = (type) =>
    `<${w}:r><${w}:fldChar ${w}:fldCharType="${type}"/></${w}:r>`;
  const instr = `<${w}:r><${w}:instrText xml:space="preserve">${code}</${w}:instrText></${w}:r>`;
  const result = `<${w}:r><${w}:t>${words(r)}</${w}:t></${w}:r>`;
  if (r.chance(0.2)) {
    return marker("begin") + instr + marker("end");
  }
  const parts = [
    marker("begin"),
    instr,
    marker("separate"),
    result,
    marker("end"),
  ];
  if (r.chance(0.15)) {
    // Unbalanced: an end or separator with no begin.
    return marker(r.pick(["end", "separate"])) + result;
  }
  if (r.chance(0.2)) {
    return `<${w}:ins${author(r)} ${w}:id="9">${parts.join("")}</${w}:ins>`;
  }
  return parts.join("");
}

function equation(r, o, depth = 0) {
  const w = o.w;
  const mr = () => {
    const inner = r.chance(0.2)
      ? `<${w}:del${author(r)}><m:t>${r.pick(["2", "x"])}</m:t></${w}:del><${w}:ins><m:t>3</m:t></${w}:ins>`
      : `<m:t>${r.pick(["x", "y", "2", "n−1", "a+b", "α"])}</m:t>`;
    return `<m:r>${inner}</m:r>`;
  };
  const e = (name) =>
    `<m:${name}>${depth < 2 && r.chance(0.3) ? equation(r, o, depth + 1) : mr()}</m:${name}>`;
  const shapes = [
    () => mr(),
    () => `<m:sSup>${e("e")}${e("sup")}</m:sSup>`,
    () => `<m:sSub>${e("e")}${e("sub")}</m:sSub>`,
    () => `<m:sSubSup>${e("e")}${e("sub")}${e("sup")}</m:sSubSup>`,
    () => `<m:f><m:fPr/>${e("num")}${e("den")}</m:f>`,
    () => `<m:rad>${r.chance(0.5) ? e("deg") : "<m:deg/>"}${e("e")}</m:rad>`,
    () =>
      `<m:d><m:dPr>${r.chance(0.5) ? '<m:begChr m:val="["/>' : ""}${r.chance(0.3) ? '<m:sepChr m:val=";"/>' : ""}</m:dPr>${e("e")}${e("e")}</m:d>`,
    () =>
      `<m:nary><m:naryPr>${r.chance(0.5) ? '<m:chr m:val="∑"/>' : ""}</m:naryPr>${e("sub")}${e("sup")}${e("e")}</m:nary>`,
    () => `<m:func><m:fName>${mr()}</m:fName>${e("e")}</m:func>`,
    () => `<m:acc><m:accPr><m:chr m:val="̃"/></m:accPr>${e("e")}</m:acc>`,
    () => `<m:bar><m:barPr><m:pos m:val="top"/></m:barPr>${e("e")}</m:bar>`,
    () => `<m:limLow>${e("e")}${e("lim")}</m:limLow>`,
    () => `<m:eqArr>${e("e")}${e("e")}</m:eqArr>`,
    () => `<m:m><m:mr>${e("e")}${e("e")}</m:mr><m:mr>${e("e")}</m:mr></m:m>`,
    () => `<${w}:ins${author(r)}>${mr()}</${w}:ins>`,
  ];
  return Array.from({ length: 1 + r.int(3) }, () => r.pick(shapes)()).join("");
}

function textBox(r, o, depth) {
  const w = o.w;
  const paragraphs = Array.from({ length: 1 + r.int(2) }, () =>
    paragraph(r, o, depth + 1)
  ).join("");
  const vml = `<${w}:r><${w}:pict><v:shape><v:textbox><${w}:txbxContent>${paragraphs}</${w}:txbxContent></v:textbox></v:shape></${w}:pict></${w}:r>`;
  if (r.chance(0.5)) {
    return vml;
  }
  return `<mc:AlternateContent><mc:Choice Requires="wps"><${w}:r><${w}:drawing><wp:anchor><a:graphic><a:graphicData><wps:wsp><wps:txbx><${w}:txbxContent>${paragraphs}</${w}:txbxContent></wps:txbx></wps:wsp></a:graphicData></a:graphic></wp:anchor></${w}:drawing></${w}:r></mc:Choice><mc:Fallback>${vml}</mc:Fallback></mc:AlternateContent>`;
}

function inline(r, o, depth) {
  const w = o.w;
  const runs = (n, deleted = false) =>
    Array.from({ length: n }, () => run(r, o, deleted)).join("");
  const kind = r.int(24);
  if (kind < 8) {
    return run(r, o);
  }
  switch (kind) {
    case 8:
      return `<${w}:ins${author(r)} ${w}:id="1">${runs(1 + r.int(2))}${r.chance(0.4) ? `<${w}:del${author(r)}>${runs(1, true)}</${w}:del>${runs(1)}` : ""}</${w}:ins>`;
    case 9:
      return `<${w}:del${author(r)} ${w}:id="2">${runs(1 + r.int(2), true)}</${w}:del>`;
    case 10:
      return r.chance(0.5)
        ? `<${w}:moveFrom${author(r)}>${runs(1, true)}</${w}:moveFrom>`
        : `<${w}:moveToRangeStart ${w}:id="7" ${w}:name="move1"/><${w}:moveTo${author(r)}>${runs(1)}</${w}:moveTo><${w}:moveToRangeEnd ${w}:id="7"/>`;
    case 11:
      return r.chance(0.85)
        ? r.pick([
            `<${w}:hyperlink r:id="rId1">${runs(1)}</${w}:hyperlink>`,
            `<${w}:hyperlink ${w}:anchor="here">${runs(1)}</${w}:hyperlink>`,
          ])
        : r.pick([
            `<${w}:hyperlink r:id="rIdMissing">${runs(1)}</${w}:hyperlink>`,
            `<${w}:hyperlink r:id="rIdMissing" ${w}:anchor="x">${runs(1)}</${w}:hyperlink>`,
          ]);
    case 12:
      return `<${w}:smartTag ${w}:element="place">${runs(1)}</${w}:smartTag>`;
    case 13:
      return `<${w}:customXml ${w}:element="party">${runs(1)}</${w}:customXml>`;
    case 14:
      return `<${w}:fldSimple ${w}:instr=" PAGE ">${runs(1)}</${w}:fldSimple>`;
    case 15: {
      const checkbox = r.chance(0.6)
        ? `<w14:checkbox><w14:checked w14:val="${r.pick(["1", "0"])}"/></w14:checkbox>`
        : "";
      return `<${w}:sdt><${w}:sdtPr>${checkbox}</${w}:sdtPr><${w}:sdtContent>${r.chance(0.7) ? runs(1) : ""}</${w}:sdtContent></${w}:sdt>`;
    }
    case 16:
      return `<${w}:bookmarkStart ${w}:id="0" ${w}:name="${r.pick(["_GoBack", "b1"])}"/><${w}:bookmarkEnd ${w}:id="0"/><${w}:proofErr ${w}:type="spellStart"/>`;
    case 17:
      return r.chance(0.5)
        ? `<m:oMath>${equation(r, o)}</m:oMath>`
        : `<m:oMathPara><m:oMath>${equation(r, o)}</m:oMath></m:oMathPara>`;
    case 18:
      return depth < 2 ? textBox(r, o, depth) : run(r, o);
    case 19:
      return `<${w}:dir ${w}:val="rtl">${runs(1)}</${w}:dir>`;
    case 20:
      return field(r, o);
    case 21:
      return `<${w}:r><w:drawing><wp:inline><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="${r.chance(0.85) ? "rId2" : "rIdNone"}"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></${w}:r>`;
    case 22:
      return `<${w}:ins${author(r)}>${`<m:oMath>${equation(r, o)}</m:oMath>`}</${w}:ins>`;
    default:
      return run(r, o);
  }
}

function paragraph(r, o, depth = 0) {
  const w = o.w;
  const properties = [];
  if (r.chance(0.2)) {
    properties.push(`<${w}:pStyle ${w}:val="Heading1"/>`);
  }
  if (r.chance(0.15)) {
    properties.push(
      `<${w}:numPr><${w}:ilvl ${w}:val="0"/><${w}:numId ${w}:val="${r.pick(["1", "2", "7"])}"/></${w}:numPr>`
    );
  }
  if (r.chance(0.12)) {
    properties.push(`<${w}:rPr><${w}:del${author(r)} ${w}:id="5"/></${w}:rPr>`);
  }
  if (r.chance(0.05)) {
    properties.push(`<${w}:rPr><${w}:ins ${w}:id="6"/></${w}:rPr>`);
  }
  const pPr = properties.length
    ? `<${w}:pPr>${properties.join("")}</${w}:pPr>`
    : "";
  const content = Array.from({ length: r.int(5) }, () =>
    inline(r, o, depth)
  ).join(sep(r));
  return `<${w}:p>${pPr}${content}</${w}:p>`;
}

/** Whitespace between elements, as pretty-printed XML has. */
function sep(r) {
  return r.chance(0.1) ? r.pick(["\n  ", " ", "\n"]) : "";
}

function cell(r, o, merge) {
  const w = o.w;
  const properties = [
    r.chance(0.2)
      ? `<${w}:gridSpan ${w}:val="${r.pick(["2", "x", ""])}"/>`
      : "",
    merge,
  ].join("");
  const paragraphs = Array.from({ length: 1 + r.int(2) }, () =>
    paragraph(r, o)
  ).join("");
  return `<${w}:tc><${w}:tcPr>${properties}</${w}:tcPr>${paragraphs}</${w}:tc>`;
}

function table(r, o) {
  const w = o.w;
  const rows = [];
  const columns = 1 + r.int(3);
  for (let n = 1 + r.int(4), row = 0; row < n; row += 1) {
    const deleted = r.chance(0.1)
      ? `<${w}:trPr><${w}:del ${w}:id="3"/></${w}:trPr>`
      : "";
    const cells = Array.from({ length: columns }, () =>
      cell(
        r,
        o,
        r.pick([
          "",
          "",
          `<${w}:vMerge ${w}:val="restart"/>`,
          `<${w}:vMerge/>`,
          `<${w}:vMerge ${w}:val="continue"/>`,
        ])
      )
    ).join("");
    const stray = r.chance(0.05)
      ? `<${w}:bookmarkStart ${w}:id="1" ${w}:name="r"/>`
      : "";
    rows.push(`<${w}:tr>${deleted}${stray}${cells}</${w}:tr>`);
  }
  return `<${w}:tbl><${w}:tblPr/><${w}:tblGrid/>${rows.join("")}</${w}:tbl>`;
}

function block(r, o) {
  const w = o.w;
  const kind = r.int(12);
  if (kind < 7) {
    return paragraph(r, o);
  }
  if (kind < 9) {
    return table(r, o);
  }
  if (kind === 9) {
    return `<${w}:sdt><${w}:sdtPr/><${w}:sdtContent>${paragraph(r, o)}</${w}:sdtContent></${w}:sdt>`;
  }
  if (kind === 10) {
    return r.pick([
      () => `<${w}:customXml ${w}:element="clause">${paragraph(r, o)}</${w}:customXml>`,
      () => `<${w}:moveFrom>${paragraph(r, o)}</${w}:moveFrom>`,
      () => `<${w}:moveTo>${paragraph(r, o)}</${w}:moveTo>`,
    ])();
  }
  return `<${w}:bookmarkStart ${w}:id="4" ${w}:name="top"/>${paragraph(r, o)}`;
}

const NAMESPACES = (w) =>
  [
    `xmlns:${w}="http://schemas.openxmlformats.org/wordprocessingml/2006/main"`,
    'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"',
    'xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math"',
    'xmlns:v="urn:schemas-microsoft-com:vml"',
    'xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"',
    'xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"',
    'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"',
    'xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"',
    'xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"',
    'xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml"',
    // `w` stays bound even when the body uses another prefix (drawings above use it).
    ...(w === "w"
      ? []
      : [
          'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"',
        ]),
  ].join(" ");

/** Breaks the part the way a damaged or hand-made file might. */
function damage(r, xml) {
  switch (r.int(8)) {
    case 0:
      return xml.replace("</w:body>", "&nbsp;</w:body>");
    case 1:
      return xml.replace(/<\/w:p>/, "");
    case 2:
      return xml.replace("<w:body>", '<w:body w:x="1" w:x="2">');
    case 3:
      return `<!DOCTYPE x>${xml}`;
    case 4:
      return xml.replace(
        "<w:body>",
        "<w:body><w:p><w:r><w:t>a < b</w:t></w:r></w:p>"
      );
    case 5:
      return xml.replace(
        "</w:body>",
        "<w:p><w:r><w:t>&#xD800;&#0;</w:t></w:r></w:p></w:body>"
      );
    case 6:
      return xml.replace(
        "<w:body>",
        "<w:body><w:p><w:r><w:t>x</w:T></w:r></w:p>"
      );
    default:
      return xml.slice(0, Math.floor(xml.length * 0.9));
  }
}

function documentCase(seed) {
  const r = rng(seed);
  const w = r.chance(0.1) ? "x" : "w";
  const commentIds = r.chance(0.4)
    ? ["0", "1", "2"].slice(0, 1 + r.int(3))
    : [];
  const o = { w, comments: commentIds };
  const body = Array.from({ length: 1 + r.int(6) }, () => block(r, o)).join(
    sep(r)
  );
  let document = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<${w}:document ${NAMESPACES(w)}><${w}:body>${body}<${w}:sectPr/></${w}:body></${w}:document>`;
  if (w === "w" && r.chance(0.08)) {
    document = damage(r, document);
  } else if (r.chance(0.04)) {
    // The whole document inside mc:AlternateContent, as some generators write it.
    const inner = document.slice(document.indexOf("<", 1));
    document = `<?xml version="1.0"?><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice Requires="x">${inner}</mc:Choice>${r.pick(["", `<mc:Fallback>${r.pick(["", " "])}${inner}</mc:Fallback>`])}</mc:AlternateContent>`;
  }
  if (r.chance(0.1)) {
    document = document.replaceAll("\n", "\r\n");
  }
  if (r.chance(0.1)) {
    document = `﻿${document}`;
  }

  const files = {
    "[Content_Types].xml": strToU8(
      '<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/></Types>'
    ),
    "_rels/.rels": strToU8(
      '<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>'
    ),
    "word/_rels/document.xml.rels": strToU8(
      '<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.test/a#b" TargetMode="External"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/></Relationships>'
    ),
    "word/document.xml": strToU8(document),
  };
  if (r.chance(0.3)) {
    files["word/numbering.xml"] = strToU8(
      `<?xml version="1.0"?><w:numbering ${NAMESPACES("w")}><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:pStyle w:val="Heading1"/></w:lvl></w:abstractNum><w:abstractNum w:abstractNumId="1"><w:numStyleLink w:val="${r.chance(0.85) ? "ListStyle" : "Missing"}"/></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num><w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num>${r.chance(0.1) ? `<w:num w:numId="7"/>` : ""}</w:numbering>`
    );
    files["word/styles.xml"] = strToU8(
      `<?xml version="1.0"?><w:styles ${NAMESPACES("w")}><w:style w:type="numbering" w:styleId="ListStyle"><w:pPr><w:numPr><w:numId w:val="${r.pick(["1", "2"])}"/></w:numPr></w:pPr></w:style></w:styles>`
    );
  }
  if (commentIds.length > 0) {
    const comments = commentIds
      .map(
        (id) =>
          `<w:comment w:id="${id}"${author(r)} w:initials="C">${sep(r)}${paragraph(r, { w: "w", comments: [] })}${r.chance(0.3) ? field(r, { w: "w", comments: [] }) : ""}</w:comment>`
      )
      .join(sep(r));
    files["word/comments.xml"] = strToU8(
      `<?xml version="1.0"?><w:comments ${NAMESPACES("w")}>${comments}</w:comments>`
    );
  }
  if (r.chance(0.2)) {
    files["word/footnotes.xml"] = strToU8(
      `<?xml version="1.0"?><w:footnotes ${NAMESPACES("w")}><w:footnote w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:id="1">${paragraph(r, { w: "w", comments: [] })}</w:footnote></w:footnotes>`
    );
  }
  if (r.chance(0.2)) {
    files["word/media/image1.png"] = new Uint8Array([137, 80, 78, 71]);
  }
  return zipSync(files, { level: r.chance(0.5) ? 0 : 6 });
}

async function viaMammoth(bytes) {
  try {
    return { ok: true, text: (await mammoth.extractRawText({ buffer: Buffer.from(bytes) })).value.toWellFormed() };
  } catch (error) {
    return { ok: false, error: String(error) };
  }
}

function viaFast(bytes) {
  try {
    return { ok: true, text: extractRawTextSync({ buffer: bytes }).value };
  } catch (error) {
    return { ok: false, error: String(error) };
  }
}

const outcomes = { text: 0, failed: 0 };
const failures = [];
for (let seed = FIRST; seed < FIRST + CASES; seed += 1) {
  const bytes = documentCase(seed);
  const want = await viaMammoth(bytes);
  const got = viaFast(bytes);
  outcomes[want.ok ? "text" : "failed"] += 1;
  if (want.ok !== got.ok || (want.ok && want.text !== got.text)) {
    failures.push(`seed ${seed}:\n  mammoth: ${JSON.stringify(want).slice(0, 300)}\n  fast:    ${JSON.stringify(got).slice(0, 300)}`);
  }
}
console.log(
  `mammoth ${version}: ${CASES} generated documents (seeds ${FIRST}..${FIRST + CASES - 1}), ` +
    `${outcomes.text} with text, ${outcomes.failed} both expected to fail, ${failures.length} differ`
);
if (failures.length) {
  console.log(failures.slice(0, 10).join("\n"));
  process.exit(1);
}
// Both outcomes must be exercised, or the generator has stopped testing failures.
if (!(outcomes.failed > 0 && outcomes.text > outcomes.failed)) {
  console.log("the generator no longer exercises both outcomes");
  process.exit(1);
}
