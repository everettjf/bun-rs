# bun-rs tutorial

A 30-minute walk-through of bun-rs 0.1. Each step is something you can paste
into a terminal and run.

## 0. Install

You need:
- macOS (any Apple Silicon or Intel) — `JavaScriptCore.framework` ships
  with the OS, no extra dependency.
- A recent Rust nightly (we pin one in `rust-toolchain.toml`, rustup will
  pull it automatically the first time you build).

```sh
git clone https://github.com/everettjf/bun-rs
cd bun-rs
cargo build --release
```

The binary ends up at `target/release/bun-rs` (about 3.5 MB). Add a shell
alias if you want:

```sh
alias bunrs="$(pwd)/target/release/bun-rs"
```

Confirm it works:

```sh
bunrs --version       # bun-rs 0.1.0
bunrs -p "1 + 1"      # 2
```

## 1. Hello world

```sh
bunrs -e "console.log('hello from bun-rs')"
```

`-e` evaluates an inline snippet. `-p` is the same but also prints the
expression's value (Node convention).

## 2. Run a TypeScript file

Create `hello.ts`:

```ts
interface Greeting {
  who: string;
  count: number;
}

function greet(g: Greeting): string {
  return `hello ${g.who} × ${g.count}`;
}

for (const g of [
  { who: "world", count: 1 },
  { who: "JSC", count: 2 },
  { who: "Rust", count: 3 },
]) {
  console.log(greet(g));
}
```

```sh
bunrs run hello.ts
# or just
bunrs hello.ts
```

Types are stripped by [oxc](https://oxc.rs/); the JS is then evaluated by
JavaScriptCore.

## 3. Multi-file ESM project

Create three files:

```ts
// math.ts
export const PI = 3.14;
export function add(a: number, b: number) { return a + b; }
```

```ts
// greet.ts
export function greet(who: string) {
  return "hello, " + who;
}
```

```ts
// app.ts
import { greet } from "./greet";
import * as math from "./math";

console.log(greet("bun-rs"));
console.log(`pi=${math.PI}  add(2,3)=${math.add(2, 3)}`);
```

```sh
bunrs app.ts
```

All ESM forms work: named imports, default, `import * as`, renamed
(`import { a as b }`), `export * from`, `export { x } from`.

### Dynamic import / top-level await

```ts
// dyn.ts
console.log("before");
const m = await import("./math");
console.log("after", m.PI);
```

```sh
bunrs dyn.ts
```

`await` at module top-level works because bun-rs wraps every module in an
`async function`.

### `import.meta`

```ts
// meta.ts
console.log(import.meta.url);       // file:///abs/path/meta.ts
console.log(import.meta.dirname);   // /abs/path
console.log(import.meta.filename);  // /abs/path/meta.ts
```

## 4. Working with files

The `node:fs` module's promises API is properly asynchronous:

```ts
// fs-demo.ts
import { promises as fs } from "node:fs";
import path from "node:path";

const dir = path.join(process.cwd(), "demo-out");
await fs.mkdir(dir, { recursive: true });
await fs.writeFile(path.join(dir, "hello.txt"), "hi");

// Two concurrent reads — they actually run in parallel on tokio's blocking pool.
const [a, b] = await Promise.all([
  fs.readFile(path.join(dir, "hello.txt"), "utf-8"),
  fs.readFile(path.join(dir, "hello.txt")),
]);
console.log("text:", a);
console.log("bytes:", b);            // a Buffer
console.log("isBuffer:", Buffer.isBuffer(b));

await fs.rm(dir, { recursive: true });
```

### Binary files

`fs.readFileSync(path)` (no encoding) returns a `Buffer` so binary data
round-trips correctly:

```ts
import fs from "node:fs";

const bytes = fs.readFileSync("/usr/bin/ls");
console.log("size:", bytes.length, "magic:", bytes[0], bytes[1], bytes[2], bytes[3]);
```

`Bun.file(path)` is the Bun-style API for the same:

```ts
const f = Bun.file("/etc/hosts");
console.log("size:", f.size);
const text = await f.text();
const json = await Bun.file("./package.json").json();
```

## 5. fetch (it's actually async)

```ts
// fetch-demo.ts
const t0 = Date.now();
setTimeout(() => console.log("timer at +" + (Date.now() - t0) + "ms"), 50);

const r = await fetch("https://httpbin.org/get?q=1");
console.log("status:", r.status);
console.log("at +" + (Date.now() - t0) + "ms");

const j = await r.json();
console.log("echoed url:", j.url);
```

Run it and you'll see the setTimeout fire while the request is in flight —
bun-rs runs HTTP on tokio in the background, the JS thread stays free.

`Response.text() / .json() / .bytes() / .arrayBuffer()` all work; binary
bodies (images, etc.) come through as a real Uint8Array, no UTF-8
round-trip.

## 6. HTTP server with `Bun.serve`

```ts
// server.ts
const server = Bun.serve({
  port: 3000,
  fetch(req) {
    const url = new URL(req.url);
    if (url.pathname === "/json") {
      return Response.json({ ok: true, path: url.pathname });
    }
    return new Response("hello from bun-rs! " + req.method + " " + url.pathname, {
      headers: { "x-runtime": "bun-rs" },
    });
  },
});
console.log(`listening on http://localhost:${server.port}`);
```

```sh
bunrs server.ts
# in another terminal:
curl -i http://localhost:3000/
curl -i http://localhost:3000/json
```

To stop the server programmatically, call `server.stop()` from JS.

> Limitation: requests are processed sequentially on the JS thread.
> If a handler does async work (an `await fetch`, etc.) the runtime
> continues servicing timers and the deferred promise mechanism
> resolves things in order. Truly concurrent handlers are P2 work.

## 7. crypto + assert + a tiny CLI

```ts
// cli.ts
import crypto from "node:crypto";
import assert from "node:assert";
import { promises as fs } from "node:fs";

const arg = process.argv[2];
if (!arg) {
  console.error("usage: bunrs cli.ts <file>");
  process.exit(2);
}

const data = await fs.readFile(arg);
const sha = crypto.createHash("sha256").update(data).digest("hex");
console.log(`${sha}  ${arg}`);

// inline test of the sha implementation against a known vector
assert.strictEqual(
  crypto.createHash("sha256").update("hello").digest("hex"),
  "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
);
```

```sh
bunrs cli.ts /etc/hosts
```

## 8. Running other processes

```ts
// run-cmd.ts
import cp from "node:child_process";

// sync
const out = cp.execSync("ls -la /tmp | head -5", { encoding: "utf-8" });
console.log(out);

// callback
cp.exec("echo callback-style", (err, stdout) => {
  if (err) throw err;
  console.log("got:", stdout.trim());
});

// detailed
const r = cp.spawnSync("uname", ["-a"]);
console.log("status:", r.status);
console.log("stdout:", r.stdout.toString().trim());
```

## 9. REPL

Just run `bunrs` with no arguments:

```
$ bunrs
bun-rs REPL (v0.1.0 on JavaScriptCore.framework). Ctrl-D to exit.
> 1 + 2
3
> let x = [1,2,3]; x.map(n=>n*2)
[ 2, 4, 6 ]
> function f(
... a,
... b
... ) { return a + b }
> f(10, 5)
15
> ^D
```

Multi-line input is detected via `JSCheckScriptSyntax` — if the snippet
doesn't parse yet, you get a `...` continuation prompt.

## 10. What to read next

- [`guide.md`](guide.md) — full reference: every API we ship, with caveats.
- [`roadmap.md`](roadmap.md) — what's planned for 0.2 / 0.3 / 1.0.
- [`plan.md`](plan.md) — the day-by-day build log.
