// A terminal chat client that connects to a WebSocket echo server.
//
// Run: bun-rs run chat.ts [wss://...]
// Type a line + Enter → it's sent to the server. The echo is printed
// asynchronously. Ctrl-D to quit.
//
// Default endpoint: wss://ws.postman-echo.com/raw (replies with whatever
// you send). Replace with your own server for real chat.

import readline from "node:readline";

const url = process.argv[2] || "wss://ws.postman-echo.com/raw";
const ws = new WebSocket(url);

const rl = readline.createInterface({
  input: process.stdin,
  output: process.stdout,
});

let isOpen = false;
ws.onopen = () => {
  isOpen = true;
  console.log(`connected to ${url}. type to send, Ctrl-D to quit.`);
  rl.write("> ");
};
ws.onmessage = (ev: any) => {
  // Clear our prompt, print the message, redraw the prompt.
  process.stdout.write("\r\x1b[2K");
  console.log(`< ${ev.data}`);
  if (isOpen) rl.write("> ");
};
ws.onerror = (e: any) => {
  console.error(`error: ${e.message}`);
};
ws.onclose = (ev: any) => {
  console.log(`\nclosed (${ev.code}${ev.reason ? " " + ev.reason : ""})`);
  isOpen = false;
  rl.close();
  process.exit(0);
};

rl.on("line", (line: string) => {
  if (ws.readyState !== WebSocket.OPEN) {
    console.log("(not connected yet)");
    return;
  }
  ws.send(line);
  rl.write("> ");
});

rl.on("close", () => {
  console.log("\nbye");
  ws.close(1000, "user quit");
});
