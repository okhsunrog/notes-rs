import { spawnSync } from "node:child_process";
import fs from "node:fs";

const [remote, output, maxChunks = "16", decodedMiB = "128"] = process.argv.slice(2);
if (
  !/^\d+$/.test(maxChunks) ||
  !/^\d+$/.test(decodedMiB) ||
  +maxChunks < 2 ||
  +maxChunks > 512 ||
  +decodedMiB < 16 ||
  +decodedMiB > 128
)
  throw new Error("invalid batch limits");
if (!/^\/data\/local\/tmp\/ink-compaction-[a-zA-Z0-9-]+$/.test(remote ?? "") || !output)
  throw new Error(
    "usage: run-compaction.mjs /data/local/tmp/ink-compaction-UNIQUE NEW.jsonl [maxChunks=16] [decodedMiB=128]",
  );
const fd = fs.openSync(output, "wx");
const adb = (args) => {
  const r = spawnSync("adb", args, { encoding: "utf8", timeout: 15 * 60 * 1000 });
  if (r.status !== 0) throw new Error(r.stdout + r.stderr + String(r.error ?? ""));
  return r.stdout;
};
const record = (r) => {
  fs.writeSync(fd, JSON.stringify(r) + "\n");
  fs.fsyncSync(fd);
};
record({
  kind: "environment",
  maxChunks: +maxChunks,
  decodedMiB: +decodedMiB,
  time: new Date().toISOString(),
  device: adb(["devices", "-l"]),
  battery: adb(["shell", "dumpsys", "battery"]),
  hashes: adb(["shell", "sha256sum", `${remote}/bench`, `${remote}/page.json`]),
});
// Two opposite orders, fresh process and database per run. Original event times,
// no real-time sleeps: this suite measures costs, not pen latency or energy.
for (const [i, policy] of [
  "none",
  "5",
  "30",
  "60",
  "exit",
  "exit",
  "60",
  "30",
  "5",
  "none",
].entries()) {
  console.log(`Starting ${i}: ${policy}`);
  const r = JSON.parse(
    adb([
      "shell",
      `${remote}/bench`,
      `${remote}/page.json`,
      `${remote}/run-${i}.db`,
      policy,
      "0",
      maxChunks,
      decodedMiB,
    ]),
  );
  record({ kind: "run", index: i, ...r });
  console.log(
    `Finished ${i}: CPU ${r.cpu_ms.toFixed(0)} ms; exit ${r.exit.total_ms.toFixed(0)} ms; geometry ${r.size.head_geometry_bytes} bytes`,
  );
}
record({
  kind: "finished",
  time: new Date().toISOString(),
  battery: adb(["shell", "dumpsys", "battery"]),
});
fs.closeSync(fd);
