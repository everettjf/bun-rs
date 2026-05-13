# bun-rs examples

Small, self-contained programs that exercise different parts of the
runtime. Each one runs against the binary at `target/release/bun-rs`.

## hello.ts

The hello-world from the tutorial. Multi-greeting TS file, no
dependencies.

```sh
bun-rs run examples/hello.ts
```

## shortener/

A working URL shortener: HTTP server backed by SQLite.

- `POST /` with body = `<url>` → `{ code, short }`
- `GET /<code>` → 302 redirect, bumps hit counter
- `GET /` → top 20 by hits

```sh
bun-rs run examples/shortener/server.ts
# In another shell:
curl -X POST -d 'https://example.com/long' http://localhost:3000/
curl -i http://localhost:3000/<code>
curl    http://localhost:3000/
```

Exercises: `Bun.serve`, `bun:sqlite`, `URL`, `Response.json`.

## dedupe/

Find duplicate files in a directory tree by content (SHA-256). Walks
sync, then farms hashing out to N Workers in parallel.

```sh
bun-rs run examples/dedupe/dedupe.ts <dir> --workers 4
```

Exercises: `node:fs`, `node:path`, `node:crypto`, `Worker`,
`postMessage`, `Promise.all` across workers.

## chat/

A terminal chat client connecting to a WebSocket echo server. Type a
line + Enter; the server echo prints asynchronously while the prompt
stays redrawn.

```sh
bun-rs run examples/chat/chat.ts
# or with your own server:
bun-rs run examples/chat/chat.ts wss://your-server/ws
```

Exercises: `WebSocket`, `node:readline`, simultaneous stdin + WS
events on the same event loop.

---

Each example is also a smoke test of a slice of the public API. If
the README in the parent dir is the marketing pitch, this dir is the
proof. If any of these break for you, please file an issue — they're
the contract we care about most.
