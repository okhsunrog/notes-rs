import type { JournalDate, Page } from "@/lib/api";

const JOURNAL_DATE_PATTERN = /^(\d{4})-(\d{2})-(\d{2})$/;

export function journalDateFromLocalDate(date: Date): JournalDate {
  const year = date.getFullYear().toString().padStart(4, "0");
  const month = (date.getMonth() + 1).toString().padStart(2, "0");
  const day = date.getDate().toString().padStart(2, "0");
  return `${year}-${month}-${day}`;
}

export function todayJournalDate(now = new Date()): JournalDate {
  return journalDateFromLocalDate(now);
}

export function parseJournalDate(value: string): JournalDate | null {
  const match = JOURNAL_DATE_PATTERN.exec(value);
  if (!match) return null;
  const [, year, month, day] = match;
  const date = localDateFromParts(Number(year), Number(month), Number(day));
  return journalDateFromLocalDate(date) === value ? value : null;
}

export function shiftJournalDate(date: JournalDate, days: number): JournalDate {
  const local = journalDateToLocalDate(date);
  local.setDate(local.getDate() + days);
  return journalDateFromLocalDate(local);
}

export function formatJournalDate(
  date: JournalDate,
  options: Intl.DateTimeFormatOptions = {
    weekday: "long",
    year: "numeric",
    month: "long",
    day: "numeric",
  },
  locales?: Intl.LocalesArgument,
): string {
  return new Intl.DateTimeFormat(locales, options).format(journalDateToLocalDate(date));
}

export function pageDisplayTitle(page: Page): string {
  return page.kind.kind === "journal"
    ? formatJournalDate(page.kind.date)
    : (page.title ?? "Untitled");
}

function journalDateToLocalDate(value: JournalDate): Date {
  const parsed = parseJournalDate(value);
  if (!parsed) throw new Error(`invalid JournalDate from typed API: ${value}`);
  const match = JOURNAL_DATE_PATTERN.exec(parsed);
  if (!match) throw new Error(`invalid JournalDate from typed API: ${value}`);
  const [, year, month, day] = match;
  return localDateFromParts(Number(year), Number(month), Number(day));
}

function localDateFromParts(year: number, month: number, day: number): Date {
  const date = new Date(0);
  date.setHours(12, 0, 0, 0);
  date.setFullYear(year, month - 1, day);
  return date;
}
