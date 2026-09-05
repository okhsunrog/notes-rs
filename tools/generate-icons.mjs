import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const source = readFileSync(new URL("../assets/brand/tangleaf.svg", import.meta.url), "utf8");

// Half-size artwork in the 108dp adaptive layer preserves the pointed leaf
// corners under launcher masks. Tauri's android_fg_scale only affects legacy icons.
writeFileSync(
  new URL("../assets/brand/android-foreground.svg", import.meta.url),
  source.replace('viewBox="0 0 560 560"', 'viewBox="-280 -280 1120 1120"'),
);
writeFileSync(new URL("../public/favicon.svg", import.meta.url), source);
execFileSync("vp", ["run", "tauri", "icon", "assets/brand/icons.json"], {
  cwd: root,
  stdio: "inherit",
});
