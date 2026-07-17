import { describe, expect, it } from "vite-plus/test";
import {
  formatJournalDate,
  journalDateFromLocalDate,
  pageDisplayTitle,
  parseJournalDate,
  shiftJournalDate,
} from "./journal-date";

describe("JournalDate UI boundary", () => {
  it("uses local civil components instead of a UTC conversion", () => {
    const localLateEvening = new Date(2026, 6, 17, 23, 59, 59);
    expect(journalDateFromLocalDate(localLateEvening)).toBe("2026-07-17");
  });

  it("moves across calendar boundaries without timestamp arithmetic", () => {
    expect(shiftJournalDate("2024-02-28", 1)).toBe("2024-02-29");
    expect(shiftJournalDate("2024-02-29", 1)).toBe("2024-03-01");
    expect(shiftJournalDate("2026-01-01", -1)).toBe("2025-12-31");
  });

  it("keeps civil dates stable across daylight-saving transitions", () => {
    const environment = (
      globalThis as typeof globalThis & {
        process: { env: Record<string, string | undefined> };
      }
    ).process.env;
    const previousTimezone = environment.TZ;
    environment.TZ = "America/New_York";
    try {
      expect(shiftJournalDate("2026-03-08", 1)).toBe("2026-03-09");
      expect(shiftJournalDate("2026-11-01", 1)).toBe("2026-11-02");
      expect(shiftJournalDate("2026-11-01", -1)).toBe("2026-10-31");
    } finally {
      if (previousTimezone === undefined) delete environment.TZ;
      else environment.TZ = previousTimezone;
    }
  });

  it("accepts only real canonical civil dates from user-facing links", () => {
    expect(parseJournalDate("2024-02-29")).toBe("2024-02-29");
    expect(parseJournalDate("2023-02-29")).toBeNull();
    expect(parseJournalDate("2026-7-17")).toBeNull();
  });

  it("formats a journal as a localized derived title", () => {
    expect(formatJournalDate("2026-07-17", { weekday: "long" }, "en")).toBe("Friday");
    expect(
      pageDisplayTitle({
        uuid: "019f0000-0000-7000-8000-000000000001",
        kind: { kind: "journal", date: "2026-07-17" },
        title: null,
        layout: "outline",
        createdAt: 0,
        updatedAt: 0,
      }),
    ).not.toBe("Untitled");
  });
});
