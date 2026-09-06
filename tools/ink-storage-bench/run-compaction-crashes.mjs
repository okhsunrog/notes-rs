import { spawnSync } from "node:child_process";
import fs from "node:fs";
const [remote, output] = process.argv.slice(2);
if (!/^\/data\/local\/tmp\/ink-compaction-[a-zA-Z0-9-]+$/.test(remote ?? "") || !output)
  throw new Error("usage: run-compaction-crashes.mjs REMOTE NEW.jsonl");
const fd = fs.openSync(output, "wx");
const run = (args) => spawnSync("adb", ["shell", ...args], { encoding: "utf8", timeout: 180000 });
for (const phase of ["before_commit", "after_commit"]) {
  const env = [
    `INK_PROFILE_SOURCE=${remote}/run-0.db`,
    `INK_PROFILE_DEST=${remote}/crash-${phase}.db`,
  ];
  const killed = run([
    ...env,
    `INK_PROFILE_KILL=${phase}`,
    "INK_PROFILE_CHUNKS=512",
    `${remote}/profile-crash`,
    "profile_recorded_exit",
    "--ignored",
    "--nocapture",
  ]);
  fs.writeFileSync(`${output}.${phase}.log`, killed.stdout + killed.stderr, { flag: "wx" });
  if (killed.status !== 137) throw new Error(`Expected SIGKILL: ${JSON.stringify(killed)}`);
  const base = killed.stdout.match(/^INK_PROFILE_BASE (.+)$/m);
  if (!base) throw new Error("Missing pre-crash revision");
  const revision = JSON.parse(base[1]).revision;
  const recovered = run([
    ...env,
    `INK_EXPECTED_REVISION=${revision}`,
    `${remote}/profile-crash`,
    "verify_recorded_recovery",
    "--ignored",
    "--nocapture",
  ]);
  fs.appendFileSync(`${output}.${phase}.log`, recovered.stdout + recovered.stderr);
  if (recovered.status !== 0) throw new Error(JSON.stringify(recovered));
  const line = recovered.stdout.match(/^INK_RECOVERY (.+)$/m);
  if (!line) throw new Error("Missing recovery validation");
  fs.writeSync(fd, JSON.stringify({ phase, ...JSON.parse(line[1]) }) + "\n");
  fs.fsyncSync(fd);
  console.log(`Recovered ${phase}: all retained history and revision verified`);
}
fs.closeSync(fd);
