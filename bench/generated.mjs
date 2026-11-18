// Large documents for the benchmark, generated deterministically: public corpora hold
// few long text documents. `pages` pages of paragraphs, runs with formatting, a table
// every few pages and numbered lists, about 3,000 characters a page.
import { strToU8, zipSync } from "fflate";

const WORDS =
  "the of and to in a is that for it as was with be by on not he this are or his from at which but have an they you were her she there would their we him been has when who will more no if out so said what up its about into than them can only other new some could time these two may then do first any my now such like our over man me even most made after also did many before must through back years where much your way well down should because each just those people how too little state good very make world still own see men work long get here between both life being under never day same another know while last might us great old year off come since against go came right used take three".split(
    " "
  );

const W = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"';

export function generatedDocx(pages) {
  let seed = 12345;
  const next = () => {
    seed = (seed * 1103515245 + 12345) & 0x7fffffff;
    return seed;
  };
  const sentence = () => {
    const n = 8 + (next() % 16);
    const words = Array.from({ length: n }, () => WORDS[next() % WORDS.length]);
    words[0] = words[0][0].toUpperCase() + words[0].slice(1);
    return `${words.join(" ")}.`;
  };
  const run = (text, bold) =>
    `<w:r>${bold ? "<w:rPr><w:b/></w:rPr>" : ""}<w:t xml:space="preserve">${text} </w:t></w:r>`;
  const paragraph = () =>
    `<w:p><w:pPr><w:pStyle w:val="BodyText"/></w:pPr>${Array.from({ length: 3 + (next() % 4) }, () => run(sentence(), next() % 7 === 0)).join("")}</w:p>`;
  const body = [];
  for (let page = 0; page < pages; page++) {
    body.push(`<w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr>${run(`Section ${page + 1}`)}</w:p>`);
    for (let i = 0; i < 5; i++) body.push(paragraph());
    if (page % 4 === 0) {
      const cell = () => `<w:tc><w:p>${run(sentence())}</w:p></w:tc>`;
      body.push(`<w:tbl>${Array.from({ length: 4 }, () => `<w:tr>${cell()}${cell()}${cell()}</w:tr>`).join("")}</w:tbl>`);
    }
    if (page % 3 === 0) {
      for (let i = 0; i < 3; i++) {
        body.push(`<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr>${run(sentence())}</w:p>`);
      }
    }
  }
  return zipSync(
    {
      "[Content_Types].xml": strToU8(
        '<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/></Types>'
      ),
      "_rels/.rels": strToU8(
        '<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="word/numbering.xml"/></Relationships>'
      ),
      "word/_rels/document.xml.rels": strToU8(
        '<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/></Relationships>'
      ),
      "word/numbering.xml": strToU8(
        `<?xml version="1.0"?><w:numbering ${W}><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>`
      ),
      "word/document.xml": strToU8(`<?xml version="1.0"?><w:document ${W}><w:body>${body.join("")}<w:sectPr/></w:body></w:document>`),
    },
    { level: 6 }
  );
}
