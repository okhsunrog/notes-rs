import { toast } from "sonner";
import { CommandFailure, isCommandError, unknownErrorMessage } from "./api";

export function formatError(error: unknown): string {
  if (error instanceof CommandFailure) {
    return error.code ? `${error.code}: ${error.message}` : error.message;
  }
  if (isCommandError(error)) {
    const commandFailure = new CommandFailure(error);
    return commandFailure.code
      ? `${commandFailure.code}: ${commandFailure.message}`
      : commandFailure.message;
  }
  if (error instanceof Error) return error.message;
  return unknownErrorMessage(error);
}

export function notifyError(context: string, error: unknown) {
  toast.error(formatError(error), { description: context });
}

export function notifySuccess(message: string) {
  toast.success(message);
}

export function notifyInfo(message: string) {
  toast.info(message);
}
