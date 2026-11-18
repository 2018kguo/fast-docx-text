// The public corpus the conformance check reads: the .docx test files of four
// permissively licensed projects, each at a pinned commit, each file checked against
// its SHA-256 in corpus.lock.json. Nothing is redistributed here; the files are
// fetched into conformance/corpus/ (ignored by git).
//
//   node conformance/fetch-corpus.mjs            fetch what is missing, verify all
//   node conformance/fetch-corpus.mjs --update   re-list the sources and rewrite the lock
//
// Sources and licenses:
//   mwilliamson/mammoth.js        BSD-2-Clause
//   python-openxml/python-docx    MIT
//   plutext/docx4j                Apache-2.0
//   apache/poi                    Apache-2.0
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";

const here = import.meta.dirname;
const lockPath = path.join(here, "corpus.lock.json");
export const corpusDir = path.join(here, "corpus");

const SOURCES = {
  "mwilliamson/mammoth.js": "BSD-2-Clause",
  "python-openxml/python-docx": "MIT",
  "plutext/docx4j": "Apache-2.0",
  "apache/poi": "Apache-2.0",
};

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
const rawUrl = (repo, commit, file) =>
  `https://raw.githubusercontent.com/${repo}/${commit}/${file.split("/").map(encodeURIComponent).join("/")}`;
export const localPath = (entry) =>
  path.join(corpusDir, entry.repo.replace("/", "__"), entry.path);

async function get(url, headers = {}) {
  for (let attempt = 1; ; attempt++) {
    const response = await fetch(url, { headers: { "user-agent": "fast-docx-text-corpus", ...headers } });
    if (response.ok) return response;
    if (attempt >= 4) throw new Error(`${response.status} ${url}`);
    await new Promise((resolve) => setTimeout(resolve, 1000 * attempt));
  }
}

async function update() {
  const headers = process.env.GH_TOKEN ? { authorization: `Bearer ${process.env.GH_TOKEN}` } : {};
  const api = async (p) => (await get(`https://api.github.com/${p}`, headers)).json();
  const sources = [];
  for (const [repo, license] of Object.entries(SOURCES)) {
    const source = { repo, commit: "", license, files: [] };
    const { default_branch } = await api(`repos/${repo}`);
    const { sha } = await api(`repos/${repo}/commits/${default_branch}`);
    source.commit = sha;
    const tree = await api(`repos/${repo}/git/trees/${sha}?recursive=1`);
    if (tree.truncated) throw new Error(`${repo}: tree listing truncated`);
    const files = tree.tree.filter((e) => e.type === "blob" && /\.docx$/i.test(e.path));
    for (const file of files) {
      const bytes = new Uint8Array(await (await get(rawUrl(repo, sha, file.path))).arrayBuffer());
      const entry = { repo, path: file.path };
      mkdirSync(path.dirname(localPath(entry)), { recursive: true });
      writeFileSync(localPath(entry), bytes);
      source.files.push([file.path, sha256(bytes)]);
    }
    sources.push(source);
    console.log(`${repo}@${sha.slice(0, 10)}: ${files.length} files (${license})`);
  }
  // One line per file, so a change reads as a diff.
  const text = sources
    .map(
      ({ files, ...source }) =>
        `  ${JSON.stringify(source).slice(0, -1).replaceAll('","', '", "').replaceAll('":"', '": "')}, "files": [\n${files
          .map((file) => `    ${JSON.stringify(file).replace('","', '", "')}`)
          .join(",\n")}\n  ]}`
    )
    .join(",\n");
  writeFileSync(lockPath, `[\n${text}\n]\n`);
}

/** Every file of the lock: { repo, commit, license, path, sha256 }. */
export function lock() {
  return JSON.parse(readFileSync(lockPath, "utf8")).flatMap(({ files, ...source }) =>
    files.map(([file, sha]) => ({ ...source, path: file, sha256: sha }))
  );
}

async function fetchAll() {
  const entries = lock();
  let fetched = 0;
  const queue = [...entries];
  await Promise.all(
    Array.from({ length: 8 }, async () => {
      for (let entry = queue.shift(); entry; entry = queue.shift()) {
        const file = localPath(entry);
        if (!existsSync(file)) {
          const bytes = new Uint8Array(await (await get(rawUrl(entry.repo, entry.commit, entry.path))).arrayBuffer());
          mkdirSync(path.dirname(file), { recursive: true });
          writeFileSync(file, bytes);
          fetched++;
        }
        if (sha256(readFileSync(file)) !== entry.sha256) {
          throw new Error(`${entry.repo}/${entry.path}: SHA-256 mismatch`);
        }
      }
    })
  );
  console.log(`${entries.length} files verified (${fetched} fetched)`);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  await (process.argv.includes("--update") ? update() : fetchAll());
}
