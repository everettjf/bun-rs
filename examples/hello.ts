// First .ts script bun-rs runs end-to-end: parse → oxc transpile → JSC eval.

interface Greeting {
  who: string;
  count: number;
}

function greet(g: Greeting): string {
  return `hello ${g.who} x${g.count}`;
}

const items: Greeting[] = [
  { who: "world", count: 1 },
  { who: "JSC", count: 2 },
  { who: "Rust", count: 3 },
];

for (const it of items) {
  console.log(greet(it));
}

console.log("argv:", process.argv);
console.log("cwd:", process.cwd());
