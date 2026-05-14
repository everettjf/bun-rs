# bun-rs tutorial

Twelve runnable steps showing what bun-rs can actually do today.
Each block is something you can paste into a terminal.

> ⚠️ bun-rs is an **experimental hobby project**. The features in this
> tutorial all work as written. **Anything you reach for outside this
> tutorial may throw, return a stub, or behave subtly differently from
> real Bun.** See [`capabilities.md`](capabilities.md) before betting on
> anything.

## 0. Build

You need:

- **macOS** — `JavaScriptCore.framework` ships with the OS. No extra deps.
- *or* **Linux** — `apt install libjavascriptcoregtk-4.1-dev` (lightly tested).
- A recent Rust nightly. We pin one in `rust-toolchain.toml`; rustup will
  fetch it automatically the first time you build.

```sh
git clone https://github.com/everettjf/bun-rs
cd bun-rs
cargo build --release
```

Binary lands at `target/release/bun-rs` (~3.5 MB). Convenience alias:

```sh
alias bunrs="$(pwd)/target/release/bun-rs"
bunrs --version
bunrs -p "1 + 1"      # 2
```

Everything below assumes `bunrs` is on your path.

## 1. Hello world

Two ways to evaluate inline:

```sh
bunrs -e "console.log('hello from bun-rs')"   # prints the message
bunrs -p "1 + 2 * 3"                          # 7  (prints the value too)
```

`-e` evaluates a snippet. `-p` does the same and prints the result. Both
wrap your snippet in an async IIFE, so top-level `await` works:

```sh
bunrs -e "const r = await fetch('https://example.com'); console.log(r.status)"
```

## 2. Run a TypeScript file

Create `hello.ts`:

```ts
interface Greeting { who: string; count: number }
const greet = ({ who, count }: Greeting) => `hello ${who} × ${count}`;

for (const g of [
  { who: "world", count: 1 },
  { who: "JSC",   count: 2 },
  { who: "Rust",  count: 3 },
]) console.log(greet(g));
```

```sh
bunrs hello.ts          # equivalent to: bunrs run hello.ts
```

Types are stripped by [oxc](https://oxc.rs/); the resulting JS is
evaluated by JavaScriptCore.

## 3. Multi-file ESM

```ts
// math.ts
export const PI = 3.14;
export function add(a: number, b: number) { return a + b }
```

```ts
// greet.ts
export const greet = (who: string) => `hello, ${who}`;
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
(`import { a as b }`), `export * from`, `export { x } from`, `export *
as ns from`.

## 4. Dynamic import + top-level await + `import.meta`

```ts
// dyn.ts
console.log("before");
const m = await import("./math");
console.log("after", m.PI);

console.log(import.meta.url);       // file:///abs/path/dyn.ts
console.log(import.meta.dirname);   // /abs/path
console.log(import.meta.filename);  // /abs/path/dyn.ts
```

```sh
bunrs dyn.ts
```

`await` at module top-level works because bun-rs wraps every module in
an `async function`. Dynamic `import()` resolves the same way static
imports do — `node_modules` lookup, conditions, the works.

## 5. Reading and writing files

`fs.promises` is **genuinely async** (tokio `spawn_blocking`), so two
concurrent reads actually overlap on the blocking pool:

```ts
// fs-demo.ts
import { promises as fs } from "node:fs";
import path from "node:path";

const dir = path.join(process.cwd(), "demo-out");
await fs.mkdir(dir, { recursive: true });
await fs.writeFile(path.join(dir, "hello.txt"), "hi");

const [a, b] = await Promise.all([
  fs.readFile(path.join(dir, "hello.txt"), "utf-8"),
  fs.readFile(path.join(dir, "hello.txt")),  // Buffer
]);
console.log("text:", a);
console.log("bytes:", b);
console.log("isBuffer:", Buffer.isBuffer(b));

await fs.rm(dir, { recursive: true });
```

`fs.readFileSync(path)` with no encoding returns a `Buffer` so binary
data round-trips correctly. `Bun.file` is the Bun-style version:

```ts
const f = Bun.file("/etc/hosts");
console.log("size:", f.size, "type:", f.type);
console.log(await f.text());
const pkg = await Bun.file("./package.json").json();
```

## 6. fetch (and yes, it's actually async)

```ts
// fetch-demo.ts
const t0 = Date.now();
setTimeout(() => console.log("timer at +" + (Date.now() - t0) + "ms"), 50);

const r = await fetch("https://httpbin.org/get?q=1");
console.log("status:", r.status, "at +" + (Date.now() - t0) + "ms");

const j = await r.json();
console.log("echoed url:", j.url);
```

Run it — the `setTimeout` fires while the request is in flight. HTTP
runs on tokio in the background; the JS thread stays free.

`Response.text() / .json() / .bytes() / .arrayBuffer()` all work; binary
bodies come through as real `Uint8Array`, no UTF-8 round-trip.

`AbortController` honours request setup; it does not interrupt
mid-stream — see [`capabilities.md`](capabilities.md).

## 7. An HTTP server

```ts
// server.ts
const server = Bun.serve({
  port: 3000,
  fetch(req) {
    const url = new URL(req.url);
    if (url.pathname === "/json") {
      return Response.json({ ok: true, path: url.pathname });
    }
    return new Response(
      `hello from bun-rs! ${req.method} ${url.pathname}`,
      { headers: { "x-runtime": "bun-rs" } },
    );
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

HTTPS works the same way — pass `tls: { cert: Bun.file("cert.pem"), key:
Bun.file("key.pem") }`. Per-request handling runs on tokio + hyper.

## 8. Spawning subprocesses

```ts
// run-cmd.ts
import cp from "node:child_process";

// sync, capture output
const out = cp.execSync("ls -la /tmp | head -5", { encoding: "utf-8" });
console.log(out);

// callback style
cp.exec("echo callback-style", (err, stdout) => {
  if (err) throw err;
  console.log("got:", stdout.trim());
});

// structured
const r = cp.spawnSync("uname", ["-a"]);
console.log("status:", r.status, "stdout:", r.stdout.toString().trim());
```

For `Bun.spawn` (async) the stdout stream now works (batch 210). Writing
to `proc.stdin` after spawn does **not** work yet — for stdin pass
`input:` to `spawnSync` instead.

## 9. Hashing + assert

```ts
// hash.ts
import crypto from "node:crypto";
import assert from "node:assert";
import { promises as fs } from "node:fs";

const arg = process.argv[2];
if (!arg) { console.error("usage: bunrs hash.ts <file>"); process.exit(2) }

const sha = crypto.createHash("sha256").update(await fs.readFile(arg)).digest("hex");
console.log(`${sha}  ${arg}`);

// inline check against a known vector
assert.strictEqual(
  crypto.createHash("sha256").update("hello").digest("hex"),
  "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
);
```

```sh
bunrs hash.ts /etc/hosts
```

## 10. The test runner

Create `math.test.ts`:

```ts
import { describe, test, expect, beforeEach, jest } from "bun:test";

function double(n: number) { return n * 2 }

describe("double", () => {
  let calls = 0;
  beforeEach(() => { calls = 0 });

  test("works on positives", () => {
    expect(double(3)).toBe(6);
    calls++;
    expect(calls).toBeGreaterThan(0);
  });

  test("matchers", () => {
    expect({ a: 1, b: 2 }).toEqual({ a: 1, b: 2 });
    expect([1, 2, 3]).toContain(2);
    expect("hello world").toMatch(/world/);
    expect(() => { throw new Error("nope") }).toThrow("nope");
  });

  test.each([[1, 2], [2, 4], [10, 20]])(
    "double(%i) → %i",
    (input, expected) => expect(double(input)).toBe(expected),
  );

  test("fake timers", () => {
    jest.useFakeTimers();
    jest.setSystemTime(new Date("2025-01-01T00:00:00Z"));
    expect(Date.now()).toBe(new Date("2025-01-01T00:00:00Z").getTime());

    let fired = false;
    setTimeout(() => { fired = true }, 1000);
    jest.advanceTimersByTime(1000);
    expect(fired).toBe(true);

    jest.useRealTimers();
  });
});
```

```sh
bunrs test                       # discovers *.test.{ts,tsx,js,jsx} recursively
bunrs test math                  # only files whose path contains "math"
bunrs test --bail 1              # stop after first failure
```

Output mirrors Bun's: `bun test <version>` header on stdout, `✓` / `✗`
on stderr, footer counters.

## 11. Bundle and install

```sh
# Single-file ESM bundle (no tree shake / plugins / minify)
bunrs build app.ts -o out.js

# Install dependencies (loose semver, no lockfile)
bunrs install
```

Treat `bun-rs install` as "fetches packages from npm" — it's enough to
get most pure-JS deps onto disk, not enough to replace `npm install` on
a real project. See [`capabilities.md`](capabilities.md) §8.

## 12. The REPL

Run `bunrs` with no arguments:

```
$ bunrs
bun-rs REPL (v0.1.0 on JavaScriptCore.framework). Ctrl-D to exit.
> 1 + 2
3
> let xs = [1,2,3]; xs.map(n => n * 2)
[ 2, 4, 6 ]
> function f(
... a,
... b,
... ) { return a + b }
> f(10, 5)
15
> ^D
```

Multi-line input is detected via `JSCheckScriptSyntax` — if the snippet
doesn't parse yet, you get a `...` continuation prompt.

---

## What to read next

- [`capabilities.md`](capabilities.md) — the candid pass/stub/fail list.
- [`guide.md`](guide.md) — fuller API reference.
- [`roadmap.md`](roadmap.md) — what's planned next.

## Things that look like they should work but don't

Quick reminder before you wander off the tutorial: bun-rs has known
sharp edges. The full list is in [`capabilities.md`](capabilities.md);
the ones that bite most often:

- `proc.stdin.write(...)` after `Bun.spawn(...)` — async stdin not yet
  exposed. Use `Bun.spawnSync({ input: ... })`.
- `import x from "./data.toml" with { type: "toml" }` — import
  attributes for non-JSON unparsed. Use the file extension only.
- `Bun.password.hash(pw, "argon2id")` — runs HMAC-SHA256 under the hood,
  not argon2. Compatible API, **incompatible bytes**.
- `new Worker(...)`, `node:worker_threads` — workers throw.
- `Bun.listen` / `Bun.connect` / `node:net.createServer` — raw TCP/UDP
  not implemented; HTTP-shaped traffic goes through hyper/reqwest.
- `node:dns.lookup` — always returns 127.0.0.1 / ::1.
- Server-side `WebSocket` upgrade in `Bun.serve` — only the client is real.
- Stack traces use JSC format (`name@url:line:col`). Tests that grep for
  V8's `at name (url:line:col)` will not match.
- `expect(x).toMatchSnapshot()` always passes — snapshot files get
  created on first call but are never diffed or rewritten.
