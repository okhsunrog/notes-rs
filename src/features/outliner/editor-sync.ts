export type RemoteDraftDecision = "unchanged" | "accept_remote" | "conflict";

export function reconcileRemoteDraft(
  previousContent: string,
  draftContent: string,
  remoteContent: string,
): RemoteDraftDecision {
  if (remoteContent === previousContent) return "unchanged";
  return draftContent === previousContent ? "accept_remote" : "conflict";
}
