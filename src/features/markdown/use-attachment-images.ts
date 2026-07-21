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

/** Builds one synchronous resolver for an already-batched page snapshot. */
export function useResolvedAttachmentImages(
  descriptors: readonly AttachmentImageDescriptor[],
): MarkdownImageResolver | undefined {
  const tauri = isTauri();
  return useMemo(() => {
    if (!tauri || descriptors.length === 0) return undefined;
    const images = new Map(
      descriptors.map((descriptor) => [descriptor.attachmentUuid, resolvedImage(descriptor)]),
    );
    return ({ attachmentUuid }) => images.get(attachmentUuid) ?? null;
  }, [descriptors, tauri]);
}

function resolvedImage(descriptor: AttachmentImageDescriptor): MarkdownResolvedImage {
  // `convertFileSrc` deliberately treats its first argument as one opaque path
  // and percent-encodes `/`. Resolve only the platform-specific origin here,
  // then append our already typed route segments ourselves.
  const origin = convertFileSrc("", "notes-attachment");
  const resource = (variant: "preview" | "original") =>
    attachmentResourceUrl(origin, descriptor, variant);
  return {
    byteSize: descriptor.byteSize,
    height: descriptor.previewHeight,
    mime: descriptor.mime,
    originalSrc: resource("original"),
    src: resource("preview"),
    width: descriptor.previewWidth,
  };
}

export function attachmentResourceUrl(
  origin: string,
  descriptor: Pick<AttachmentImageDescriptor, "attachmentUuid" | "blobHash" | "resourceVersion">,
  variant: "preview" | "original",
): string {
  const base = origin.endsWith("/") ? origin : `${origin}/`;
  return `${base}v${descriptor.resourceVersion}/${encodeURIComponent(descriptor.attachmentUuid)}/${encodeURIComponent(descriptor.blobHash)}/${variant}`;
}
