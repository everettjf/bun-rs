// Worker: hash a list of file paths and report results back.
import fs from "node:fs";
import crypto from "node:crypto";

onmessage = (e: any) => {
  if (e.data.kind !== "hash") return;
  const files: string[] = e.data.files;
  const results = files.map((path) => {
    const data = fs.readFileSync(path);
    const hash = crypto.createHash("sha256").update(data).digest("hex");
    const size = data.length;
    return { path, hash, size };
  });
  postMessage({ kind: "done", results });
};
