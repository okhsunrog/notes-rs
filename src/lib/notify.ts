import { toast } from "sonner";
import { CommandFailure, isCommandError, unknownErrorMessage } from "./api";

export function formatError(error: unknown): string {
  if (error instanceof CommandFailure || isCommandError(error)) {
    return `${error.code}: ${error.message}`;
  }
  if (error instanceof Error) {
    if (!error.message) return String(error);
    return error.name === "Error" ? error.message : `${error.name}: ${error.message}`;
  }
  return unknownErrorMessage(error);
}

/** Errors stay visible twice as long as the default so a glance away doesn't miss them. */
const ERROR_TOAST_DURATION_MS = 8_000;

export function notifyError(context: string, error: unknown) {
  toast.error(`${context} error: ${formatError(error)}`, { duration: ERROR_TOAST_DURATION_MS });
}

export function notifySuccess(message: string) {
  toast.success(message);
}

export function notifyInfo(message: string) {
  toast.info(message);
}
