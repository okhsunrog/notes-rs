import { beforeEach, describe, expect, it, vi } from "vite-plus/test";
import { CommandFailure } from "./api";
import { formatError, notifyError } from "./notify";

const { toastError } = vi.hoisted(() => ({ toastError: vi.fn() }));

vi.mock("sonner", () => ({
  toast: {
    error: toastError,
    info: vi.fn(),
    success: vi.fn(),
  },
}));

describe("notify", () => {
  beforeEach(() => {
    toastError.mockClear();
  });

  it("formats command failures without losing their code", () => {
    expect(
      formatError(new CommandFailure({ code: "not_found", message: "page was not found" })),
    ).toBe("not_found: page was not found");
  });

  it("keeps the name of named errors", () => {
    expect(formatError(new TypeError("x is not a function"))).toBe(
      "TypeError: x is not a function",
    );
  });

  it("never renders an empty-message error as blank", () => {
    expect(formatError(new Error())).toBe("Error");
  });

  it("preserves the contextual error message and extends error visibility", () => {
    notifyError("open", new Error("page was not found"));

    expect(toastError).toHaveBeenCalledWith("open error: page was not found", {
      duration: 8_000,
    });
  });
});
