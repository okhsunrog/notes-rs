import { describe, expect, it } from "vite-plus/test";
import { CommandFailure, unwrapCommand } from "./api";

describe("typed command results", () => {
  it("unwraps successful generated command results", async () => {
    await expect(unwrapCommand(Promise.resolve({ status: "ok", data: 42 }))).resolves.toBe(42);
  });

  it("preserves the typed backend error code", async () => {
    const failure = unwrapCommand(
      Promise.resolve({
        status: "error",
        error: { code: "conflict", message: "request is already active" },
      }),
    );

    await expect(failure).rejects.toMatchObject({
      name: "CommandFailure",
      code: "conflict",
      message: "request is already active",
    } satisfies Partial<CommandFailure>);
  });

  it("preserves raw Tauri argument deserialization errors as ordinary errors", async () => {
    const failure = unwrapCommand(
      Promise.resolve({
        status: "error",
        error: "invalid URL: relative URL without a base",
      }),
    );

    await expect(failure).rejects.toEqual(new Error("invalid URL: relative URL without a base"));
    await expect(failure).rejects.not.toBeInstanceOf(CommandFailure);
  });
});
