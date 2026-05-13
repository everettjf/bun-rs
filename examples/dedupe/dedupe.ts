// Find duplicate files in a directory tree by content.
//
// Walks the directory (sync, on the main thread), then farms hashing
// out to N workers in parallel. Prints groups of files sharing a
// SHA-256.
//
// Run: bun-rs run dedupe.ts <dir> [--workers N]

import fs from "node:fs";
import path from "node:path";

const NUM_WORKERS = (() => {
  const i = process.argv.indexOf("--workers");
  if (i >= 0 && i + 1 < process.argv.length) return Number(process.argv[i + 1]);
  return 4;
})();

const root = process.argv[2];
if (!root) {
  console.error("usage: bun-rs run dedupe.ts <dir> [--workers N]");
  process.exit(2);
}

function walk(dir: string, out: string[] = []): string[] {
  for (const name of fs.readdirSync(dir)) {
    if (name.startsWith(".")) continue;
    const p = path.join(dir, name);
    const st = fs.statSync(p);
    if (st.isDirectory()) walk(p, out);
    else if (st.isFile()) out.push(p);
  }
  return out;
}

const t0 = Date.now();
const files = walk(root);
console.log(`found ${files.length} files in ${Date.now() - t0}ms`);

// Hand chunks to N workers. Each worker hashes its chunk and reports back.
const chunkSize = Math.ceil(files.length / NUM_WORKERS);
const chunks: string[][] = [];
for (let i = 0; i < files.length; i += chunkSize) {
  chunks.push(files.slice(i, i + chunkSize));
}

type Result = { path: string; hash: string; size: number };

const workerPath = path.join(import.meta.dirname, "worker.ts");
const tStart = Date.now();
const allResults = await Promise.all(
  chunks.map(
    (chunk, idx) =>
      new Promise<Result[]>((resolve, reject) => {
        const w = new Worker(workerPath);
        w.onmessage = (ev: any) => {
          if (ev.data.kind === "done") {
            w.terminate();
            resolve(ev.data.results);
          }
        };
        w.onerror = (e: any) => {
          w.terminate();
          reject(new Error("worker " + idx + ": " + e.message));
        };
        w.postMessage({ kind: "hash", files: chunk });
      })
  )
);
const elapsed = Date.now() - tStart;

const results: Result[] = allResults.flat();
console.log(`hashed ${results.length} files in ${elapsed}ms across ${NUM_WORKERS} workers`);

// Group by hash. Skip 1-element groups (no dupes).
const byHash = new Map<string, Result[]>();
for (const r of results) {
  if (!byHash.has(r.hash)) byHash.set(r.hash, []);
  byHash.get(r.hash)!.push(r);
}

const groups = [...byHash.values()].filter(g => g.length > 1);
groups.sort((a, b) => b[0].size - a[0].size);

if (groups.length === 0) {
  console.log("\nno duplicates found.");
} else {
  console.log(`\nfound ${groups.length} duplicate groups:`);
  for (const g of groups) {
    console.log(`\n  ${g[0].hash.slice(0, 16)}…  (${g[0].size} bytes × ${g.length} files)`);
    for (const r of g) console.log(`    ${r.path}`);
  }
}
