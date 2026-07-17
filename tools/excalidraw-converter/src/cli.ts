#!/usr/bin/env node

import { parseArguments, usage } from "./args.ts";
import { convertDrawings } from "./convert.ts";

async function main(): Promise<void> {
  let options;
  try {
    options = parseArguments(process.argv.slice(2));
  } catch (error: unknown) {
    console.error(messageOf(error));
    console.error(usage());
    process.exitCode = 2;
    return;
  }

  if ("help" in options) {
    console.log(usage());
    return;
  }

  try {
    const bundle = await convertDrawings(options);
    const converted = bundle.drawings.filter((drawing) => drawing.status === "converted").length;
    const skipped = bundle.drawings.filter((drawing) => drawing.status === "skipped_empty").length;
    const failed = bundle.drawings.filter((drawing) => drawing.status === "failed").length;
    console.log(`converted=${converted} skipped_empty=${skipped} failed=${failed}`);
    if (failed > 0) {
      process.exitCode = 1;
    }
  } catch (error: unknown) {
    console.error(`conversion did not publish: ${messageOf(error)}`);
    process.exitCode = 1;
  }
}

await main();

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : "unknown error";
}
