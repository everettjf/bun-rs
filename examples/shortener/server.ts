// A tiny URL shortener.
//
// Run:
//   bun-rs run server.ts
// Use:
//   curl -X POST -d 'https://example.com/very/long/url' http://localhost:3000/
//   curl -v http://localhost:3000/<code>
//
// Storage: SQLite (file-backed; survives restarts).

import { Database } from "bun:sqlite";
import path from "node:path";

const db = new Database(path.join(import.meta.dirname, "urls.db"));
db.run(`
  CREATE TABLE IF NOT EXISTS urls (
    code TEXT PRIMARY KEY,
    target TEXT NOT NULL,
    hits INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
  )
`);

const insert = db.query(
  "INSERT OR IGNORE INTO urls (code, target) VALUES (?, ?)"
);
const fetchByCode = db.query("SELECT target, hits FROM urls WHERE code = ?");
const bump = db.query("UPDATE urls SET hits = hits + 1 WHERE code = ?");
const list = db.query("SELECT code, target, hits FROM urls ORDER BY hits DESC LIMIT 20");

// 6-char base32-ish code.
const ALPHA = "23456789abcdefghjkmnpqrstuvwxyz";
function makeCode(): string {
  let out = "";
  for (let i = 0; i < 6; i++) {
    out += ALPHA[Math.floor(Math.random() * ALPHA.length)];
  }
  return out;
}

function shortenOne(target: string): string {
  // Try a few times to avoid (very unlikely) collisions.
  for (let i = 0; i < 5; i++) {
    const code = makeCode();
    const r = insert.run(code, target);
    if (r.changes > 0) return code;
  }
  throw new Error("could not allocate code");
}

const server = Bun.serve({
  port: 3000,
  async fetch(req) {
    const url = new URL(req.url);
    const method = req.method;

    // Create: POST / with body = target URL
    if (method === "POST" && url.pathname === "/") {
      const target = (await req.text()).trim();
      if (!target.startsWith("http")) {
        return new Response("body must be an http(s) URL\n", { status: 400 });
      }
      const code = shortenOne(target);
      return Response.json({ code, short: `http://localhost:${server.port}/${code}` }, { status: 201 });
    }

    // List: GET /
    if (method === "GET" && url.pathname === "/") {
      const rows = list.all();
      return Response.json({ urls: rows });
    }

    // Redirect: GET /<code>
    if (method === "GET" && url.pathname.length > 1) {
      const code = url.pathname.slice(1);
      const row = fetchByCode.get(code) as { target: string; hits: number } | undefined;
      if (!row) return new Response("not found\n", { status: 404 });
      bump.run(code);
      return new Response(null, {
        status: 302,
        headers: { location: row.target },
      });
    }

    return new Response("method not allowed\n", { status: 405 });
  },
});

console.log(`listening on http://localhost:${server.port}`);
console.log(`  POST /                  body=<url> → { code, short }`);
console.log(`  GET  /<code>            302 → target, bumps hits`);
console.log(`  GET  /                  top 20 by hits`);
