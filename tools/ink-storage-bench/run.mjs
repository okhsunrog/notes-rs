import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
// Run from the host. The binary and private corpus must already be on the device.
const [remote, output, pacing = "0", order = "forward"] = process.argv.slice(2);
if (!remote?.startsWith("/data/local/tmp/ink-policy-") || !output)
  throw new Error(
    "usage: node run.mjs /data/local/tmp/ink-policy-UNIQUE output.jsonl [0|1] [forward|reverse]",
  );
if (!["0", "1"].includes(pacing) || !["forward", "reverse"].includes(order))
  throw new Error("Invalid pacing or order");
const fd = fs.openSync(output, "wx");
const adb = (args) => spawnSync("adb", args, { encoding: "utf8", timeout: 20 * 60 * 1000 });
const run = (mode, name, phase) => {
  const args = [remote + "/bench", remote + "/page.json", remote + "/" + name + ".db", mode];
  if (mode !== "verify") args.push(pacing);
  if (phase) args.push(phase);
  return adb(["shell", ...args]);
};
const record = (value) => {
  fs.writeSync(fd, JSON.stringify(value) + "\n");
  fs.fsyncSync(fd);
};
record({
  kind: "environment",
  time: new Date().toISOString(),
  pacing,
  order,
  device: adb(["devices", "-l"]).stdout,
  battery: adb(["shell", "dumpsys", "battery"]).stdout,
  binary: adb(["shell", "sha256sum", remote + "/bench", remote + "/page.json"]).stdout,
});
const modes = pacing === "1" ? ["raw", "pco"] : ["pco", "raw", "raw", "pco", "pco", "raw"];
if (order === "reverse") modes.reverse();
for (const [i, mode] of modes.entries()) {
  const name = `run-${pacing}-${i}-${mode}`;
  console.log(`Starting ${name}`);
  const result = run(mode, name);
  if (result.status !== 0)
    throw new Error(result.stderr + result.stdout + String(result.error ?? ""));
  const value = JSON.parse(result.stdout.trim());
  record({ kind: "run", name, ...value });
  console.log(
    `Finished ${name}: CPU ${value.cpu_ms.toFixed(1)} ms; stroke p95 ${value.stroke.p95_ms.toFixed(2)} ms; autosave p95 ${value.autosave.p95_ms.toFixed(2)} ms`,
  );
}
if (pacing === "0")
  for (const mode of ["pco", "raw"])
    for (const phase of ["before_pack", "after_encode", "before_commit", "after_commit"]) {
      const name = `crash-${mode}-${phase}`;
      const killed = run(mode, name, phase);
      if (killed.status !== 137)
        throw new Error(`Expected SIGKILL exit 137: ${JSON.stringify(killed)}`);
      const recovered = run("verify", name);
      if (recovered.status !== 0) throw new Error(recovered.stderr + recovered.stdout);
      const verified = JSON.parse(recovered.stdout.trim());
      const expected = Number(
        (killed.stdout + killed.stderr).match(/crash_expected_strokes=(\d+)/)?.[1],
      );
      if (!(expected > 0) || verified.verified_strokes !== expected)
        throw new Error(
          `Recovery lost committed strokes: expected ${expected}, got ${verified.verified_strokes}`,
        );
      record({ kind: "crash", mode, phase, ...verified });
      console.log(`Recovered ${name}: ${verified.verified_strokes} strokes`);
    }
record({
  kind: "finished",
  time: new Date().toISOString(),
  battery: adb(["shell", "dumpsys", "battery"]).stdout,
});
fs.closeSync(fd);
console.log(`Results: ${path.resolve(output)}`);
