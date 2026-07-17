import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { convertFileSrc, isTauri } from "@tauri-apps/api/core";
import { resolveAttachmentImages, type AttachmentImageDescriptor } from "@/lib/api";
import { queryKeys } from "@/lib/query";
import {
  extractMarkdownAttachmentUuids,
  type MarkdownImageResolver,
  type MarkdownResolvedImage,
} from "./image-policy";

export function useAttachmentImageResolver(markdown: string): MarkdownImageResolver | undefined {
  const attachmentUuids = useMemo(() => extractMarkdownAttachmentUuids(markdown), [markdown]);
  const tauri = isTauri();
  const descriptors = useQuery({
    queryKey: queryKeys.attachmentImages(attachmentUuids),
    queryFn: () => resolveAttachmentImages(attachmentUuids),
    enabled: tauri && attachmentUuids.length > 0,
  });

  return useMemo(() => {
    if (!tauri || attachmentUuids.length === 0) return undefined;
    const images = new Map(
      (descriptors.data ?? []).map((descriptor) => [
        descriptor.attachmentUuid,
        resolvedImage(descriptor),
      ]),
    );
    return ({ attachmentUuid }) => images.get(attachmentUuid) ?? null;
  }, [attachmentUuids, descriptors.data, tauri]);
}

function resolvedImage(descriptor: AttachmentImageDescriptor): MarkdownResolvedImage {
  return {
    byteSize: descriptor.byteSize,
    height: descriptor.height,
    mime: descriptor.mime,
    src: convertFileSrc(descriptor.attachmentUuid, "notes-attachment"),
    width: descriptor.width,
  };
}
