use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;
use std::ops::Range;

use data_url::DataUrl;
use data_url::forgiving_base64::DecodeError;
use image::{ImageFormat, ImageReader, Limits};
use percent_encoding::percent_decode_str;
use pulldown_cmark::{Event, LinkType, Options, Parser, Tag};
use sha2::{Digest, Sha256};

use crate::prepared::{
    ImportInlineImageMime, ImportMarkdownRange, ImportMediaBlockedReason, ImportMediaKind,
    ImportMediaOwner, ImportMediaReference, ImportMediaResolution, ImportMediaUnsupportedReason,
    ImportRemoteMediaScheme,
};
use crate::{
    DiagnosticCode, GraphManifest, ImportDiagnostic, LogseqMarkdownSourceLine, ManifestEntry,
    Sha256Digest, SourceKind, SourcePosition, SourceRange,
};

#[derive(Debug, Clone, Copy)]
pub(crate) struct MediaLimits {
    pub max_references: u64,
    pub max_references_per_document: u64,
    pub max_reference_bytes: u64,
    pub max_inline_decoded_bytes: u64,
    pub max_inline_image_width: u32,
    pub max_inline_image_height: u32,
    pub max_inline_image_pixels: u64,
    pub max_diagnostics: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct MediaOwnerInput<'input> {
    pub relative_path: &'input str,
    pub document_source: &'input str,
    pub owner: ImportMediaOwner,
    pub original_markdown: &'input str,
    pub final_markdown: &'input str,
    pub source_lines: &'input [LogseqMarkdownSourceLine],
    pub removed_source_range: Option<SourceRange>,
}

#[derive(Debug)]
pub(crate) enum MediaCollectionError {
    InvalidSourceMapping {
        relative_path: String,
    },
    ReferenceLimitExceeded {
        relative_path: String,
        limit: u64,
    },
    TotalReferenceLimitExceeded {
        limit: u64,
    },
    ReferenceTooLong {
        relative_path: String,
        limit_bytes: u64,
    },
    DiagnosticLimitExceeded {
        limit: u64,
    },
}

#[derive(Debug)]
pub(crate) struct MediaCollection {
    pub references: Vec<ImportMediaReference>,
    pub unreferenced_asset_count: u64,
    pub unreferenced_drawing_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedMediaToken {
    kind: ImportMediaKind,
    destination: String,
    title: String,
    range: Range<usize>,
    destination_range: Option<Range<usize>>,
}

pub(crate) fn collect_media(
    inputs: &[MediaOwnerInput<'_>],
    manifest: &GraphManifest,
    limits: MediaLimits,
    diagnostics: &mut Vec<ImportDiagnostic>,
) -> Result<MediaCollection, MediaCollectionError> {
    let manifest_entries = manifest
        .entries()
        .iter()
        .map(|entry| (entry.relative_path.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut inputs_by_document = BTreeMap::<&str, Vec<usize>>::new();
    for (index, input) in inputs.iter().enumerate() {
        inputs_by_document
            .entry(input.relative_path)
            .or_default()
            .push(index);
    }

    let mut references = Vec::new();
    let mut referenced_assets = BTreeSet::new();
    let mut referenced_drawings = BTreeSet::new();
    let mut remaining_references = limits.max_references;
    let mut remaining_diagnostics = limits
        .max_diagnostics
        .checked_sub(diagnostics.len() as u64)
        .ok_or(MediaCollectionError::DiagnosticLimitExceeded {
            limit: limits.max_diagnostics,
        })?;
    for indices in inputs_by_document.values() {
        let document_references = collect_document_media(
            inputs,
            indices,
            &manifest_entries,
            limits,
            &mut remaining_references,
            &mut remaining_diagnostics,
            diagnostics,
        )?;
        for reference in &document_references {
            if let ImportMediaResolution::LocalManifest { relative_path, .. } =
                &reference.resolution
            {
                match reference.kind {
                    ImportMediaKind::MarkdownImage => {
                        referenced_assets.insert(relative_path.clone());
                    }
                    ImportMediaKind::LegacyExcalidraw => {
                        referenced_drawings.insert(relative_path.clone());
                    }
                }
            }
        }
        references.extend(document_references);
    }

    references.sort_by(|left, right| {
        left.relative_path
            .cmp(&right.relative_path)
            .then(
                left.source_range
                    .start
                    .byte_offset
                    .cmp(&right.source_range.start.byte_offset),
            )
            .then(left.kind.cmp(&right.kind))
    });
    let manifest_assets = manifest
        .entries()
        .iter()
        .filter(|entry| entry.kind == SourceKind::Asset)
        .map(|entry| entry.relative_path.as_str())
        .collect::<BTreeSet<_>>();
    let manifest_drawings = manifest
        .entries()
        .iter()
        .filter(|entry| entry.kind == SourceKind::Drawing)
        .map(|entry| entry.relative_path.as_str())
        .collect::<BTreeSet<_>>();

    Ok(MediaCollection {
        references,
        unreferenced_asset_count: manifest_assets
            .difference(
                &referenced_assets
                    .iter()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>(),
            )
            .count() as u64,
        unreferenced_drawing_count: manifest_drawings
            .difference(
                &referenced_drawings
                    .iter()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>(),
            )
            .count() as u64,
    })
}

fn collect_document_media(
    inputs: &[MediaOwnerInput<'_>],
    indices: &[usize],
    manifest_entries: &BTreeMap<&str, &ManifestEntry>,
    limits: MediaLimits,
    remaining_references: &mut u64,
    remaining_diagnostics: &mut u64,
    diagnostics: &mut Vec<ImportDiagnostic>,
) -> Result<Vec<ImportMediaReference>, MediaCollectionError> {
    let relative_path = inputs[indices[0]].relative_path;
    let mut source_cursor = SourceCursor::new(inputs[indices[0]].document_source, relative_path);
    let mut output = Vec::new();

    for input_index in indices {
        let input = &inputs[*input_index];
        let original_tokens = parse_media_tokens(input.original_markdown);
        let final_tokens = parse_media_tokens(input.final_markdown);
        let mut original = original_tokens
            .iter()
            .map(|token| {
                let absolute = map_owner_range(
                    token.range.clone(),
                    input.original_markdown,
                    input.source_lines,
                    input.document_source,
                    input.relative_path,
                )?;
                let source_range = source_cursor.range(absolute.clone())?;
                Ok((token, absolute, source_range))
            })
            .collect::<Result<Vec<_>, MediaCollectionError>>()?;
        if let Some(removed) = input.removed_source_range {
            original.retain(|(_, _, range)| !ranges_overlap(*range, removed));
        }
        if original.len() != final_tokens.len()
            || original
                .iter()
                .zip(final_tokens.iter())
                .any(|((source, _, _), final_token)| !same_semantics(source, final_token))
        {
            return Err(MediaCollectionError::InvalidSourceMapping {
                relative_path: input.relative_path.to_owned(),
            });
        }

        for ((source_token, absolute, source_range), final_token) in
            original.into_iter().zip(final_tokens.iter())
        {
            let source_bytes = absolute
                .end
                .checked_sub(absolute.start)
                .ok_or_else(|| invalid_mapping(input.relative_path))?
                as u64;
            let owner_bytes = final_token
                .range
                .end
                .checked_sub(final_token.range.start)
                .ok_or_else(|| invalid_mapping(input.relative_path))?
                as u64;
            if source_bytes > limits.max_reference_bytes || owner_bytes > limits.max_reference_bytes
            {
                return Err(MediaCollectionError::ReferenceTooLong {
                    relative_path: input.relative_path.to_owned(),
                    limit_bytes: limits.max_reference_bytes,
                });
            }
            if output.len() as u64 >= limits.max_references_per_document {
                return Err(MediaCollectionError::ReferenceLimitExceeded {
                    relative_path: input.relative_path.to_owned(),
                    limit: limits.max_references_per_document,
                });
            }
            if *remaining_references == 0 {
                return Err(MediaCollectionError::TotalReferenceLimitExceeded {
                    limit: limits.max_references,
                });
            }
            let resolution = classify_destination(
                input.relative_path,
                source_token.kind,
                &source_token.destination,
                manifest_entries,
                limits,
            );
            push_resolution_diagnostic(
                input.relative_path,
                source_range,
                &resolution,
                limits.max_diagnostics,
                remaining_diagnostics,
                diagnostics,
            )?;
            output.push(ImportMediaReference {
                relative_path: input.relative_path.to_owned(),
                owner: input.owner,
                kind: source_token.kind,
                raw_spelling: input.document_source[absolute].to_owned(),
                owner_markdown_spelling: input.final_markdown[final_token.range.clone()].to_owned(),
                title: final_token.title.clone(),
                owner_markdown_range: ImportMarkdownRange {
                    start_byte: final_token.range.start as u64,
                    end_byte: final_token.range.end as u64,
                },
                owner_markdown_destination_range: final_token.destination_range.as_ref().map(
                    |range| ImportMarkdownRange {
                        start_byte: range.start as u64,
                        end_byte: range.end as u64,
                    },
                ),
                source_range,
                resolution,
            });
            *remaining_references -= 1;
        }
    }
    Ok(output)
}

fn parse_media_tokens(markdown: &str) -> Vec<ParsedMediaToken> {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM
        | Options::ENABLE_WIKILINKS;
    let mut output = Vec::new();
    for (event, range) in Parser::new_ext(markdown, options).into_offset_iter() {
        let token = match event {
            Event::Start(Tag::Image {
                dest_url, title, ..
            }) => Some(ParsedMediaToken {
                kind: ImportMediaKind::MarkdownImage,
                destination: dest_url.into_string(),
                title: title.into_string(),
                range: range.clone(),
                destination_range: direct_image_destination_range(markdown, &range),
            }),
            Event::Start(Tag::Link {
                link_type: LinkType::WikiLink { .. },
                dest_url,
                title,
                ..
            }) if is_legacy_excalidraw_destination(&dest_url) => Some(ParsedMediaToken {
                kind: ImportMediaKind::LegacyExcalidraw,
                destination: dest_url.into_string(),
                title: title.into_string(),
                range: range.clone(),
                destination_range: None,
            }),
            _ => None,
        };
        let Some(token) = token else {
            continue;
        };
        output.push(token);
    }
    output
}

fn direct_image_destination_range(markdown: &str, token: &Range<usize>) -> Option<Range<usize>> {
    let bytes = markdown.as_bytes();
    if bytes.get(token.start..token.start.checked_add(2)?) != Some(b"![") {
        return None;
    }
    let mut cursor = token.start + 2;
    let mut label_depth = 0_u64;
    let mut escaped = false;
    loop {
        let byte = *bytes.get(cursor)?;
        if escaped {
            escaped = false;
        } else {
            match byte {
                b'\\' => escaped = true,
                b'[' => label_depth = label_depth.checked_add(1)?,
                b']' if label_depth == 0 => break,
                b']' => label_depth -= 1,
                _ => {}
            }
        }
        cursor += 1;
    }
    cursor += 1;
    if bytes.get(cursor) != Some(&b'(') {
        return None;
    }
    cursor += 1;
    while bytes
        .get(cursor)
        .is_some_and(|byte| byte.is_ascii_whitespace())
    {
        cursor += 1;
    }
    if bytes.get(cursor) == Some(&b'<') {
        cursor += 1;
        let start = cursor;
        let mut escaped = false;
        loop {
            let byte = *bytes.get(cursor)?;
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'>' {
                return Some(start..cursor);
            } else if matches!(byte, b'\n' | b'\r' | b'<') {
                return None;
            }
            cursor += 1;
        }
    }
    let start = cursor;
    let mut depth = 0_u64;
    let mut escaped = false;
    loop {
        let byte = *bytes.get(cursor)?;
        if escaped {
            escaped = false;
        } else {
            match byte {
                b'\\' => escaped = true,
                b'(' => depth = depth.checked_add(1)?,
                b')' if depth == 0 => return (start < cursor).then_some(start..cursor),
                b')' => depth -= 1,
                byte if byte.is_ascii_whitespace() && depth == 0 => {
                    return (start < cursor).then_some(start..cursor);
                }
                _ => {}
            }
        }
        cursor += 1;
    }
}

fn same_semantics(left: &ParsedMediaToken, right: &ParsedMediaToken) -> bool {
    left.kind == right.kind && left.destination == right.destination && left.title == right.title
}

fn map_owner_range(
    range: Range<usize>,
    markdown: &str,
    source_lines: &[LogseqMarkdownSourceLine],
    source: &str,
    relative_path: &str,
) -> Result<Range<usize>, MediaCollectionError> {
    if range.start > range.end
        || range.end > markdown.len()
        || !markdown.is_char_boundary(range.start)
        || !markdown.is_char_boundary(range.end)
    {
        return Err(invalid_mapping(relative_path));
    }
    let start = map_owner_boundary(range.start, markdown, source_lines, source, relative_path)?;
    let end = map_owner_boundary(range.end, markdown, source_lines, source, relative_path)?;
    if start > end || !source.is_char_boundary(start) || !source.is_char_boundary(end) {
        return Err(invalid_mapping(relative_path));
    }
    Ok(start..end)
}

fn map_owner_boundary(
    offset: usize,
    markdown: &str,
    source_lines: &[LogseqMarkdownSourceLine],
    source: &str,
    relative_path: &str,
) -> Result<usize, MediaCollectionError> {
    let line = source_lines
        .iter()
        .find(|line| {
            usize::try_from(line.markdown_start_byte).is_ok_and(|start| start <= offset)
                && usize::try_from(line.markdown_end_byte).is_ok_and(|end| offset <= end)
        })
        .ok_or_else(|| invalid_mapping(relative_path))?;
    let markdown_start =
        usize::try_from(line.markdown_start_byte).map_err(|_| invalid_mapping(relative_path))?;
    let source_start = usize::try_from(line.source_range.start.byte_offset)
        .map_err(|_| invalid_mapping(relative_path))?;
    let delta = offset
        .checked_sub(markdown_start)
        .ok_or_else(|| invalid_mapping(relative_path))?;
    let source_offset = source_start
        .checked_add(delta)
        .ok_or_else(|| invalid_mapping(relative_path))?;
    if source_offset > source.len()
        || !source.is_char_boundary(source_offset)
        || markdown.get(markdown_start..offset) != source.get(source_start..source_offset)
    {
        return Err(invalid_mapping(relative_path));
    }
    Ok(source_offset)
}

fn invalid_mapping(relative_path: &str) -> MediaCollectionError {
    MediaCollectionError::InvalidSourceMapping {
        relative_path: relative_path.to_owned(),
    }
}

fn ranges_overlap(left: SourceRange, right: SourceRange) -> bool {
    left.start.byte_offset < right.end.byte_offset && right.start.byte_offset < left.end.byte_offset
}

fn classify_destination(
    document_path: &str,
    kind: ImportMediaKind,
    destination: &str,
    manifest_entries: &BTreeMap<&str, &ManifestEntry>,
    limits: MediaLimits,
) -> ImportMediaResolution {
    if destination.chars().any(is_control) {
        return ImportMediaResolution::Blocked {
            reason: ImportMediaBlockedReason::ControlCharacter,
        };
    }
    if is_windows_or_unc_path(destination) {
        return ImportMediaResolution::Blocked {
            reason: ImportMediaBlockedReason::WindowsOrUncPath,
        };
    }
    if destination.contains('\\') {
        return ImportMediaResolution::Blocked {
            reason: ImportMediaBlockedReason::Backslash,
        };
    }
    if destination.starts_with("//") {
        return ImportMediaResolution::Blocked {
            reason: ImportMediaBlockedReason::ProtocolRelative,
        };
    }
    if destination.starts_with('/') {
        return ImportMediaResolution::Blocked {
            reason: ImportMediaBlockedReason::AbsolutePath,
        };
    }
    if let Some(scheme) = uri_scheme(destination) {
        if scheme.eq_ignore_ascii_case("http") {
            return ImportMediaResolution::RemoteBlocked {
                scheme: ImportRemoteMediaScheme::Http,
            };
        }
        if scheme.eq_ignore_ascii_case("https") {
            return ImportMediaResolution::RemoteBlocked {
                scheme: ImportRemoteMediaScheme::Https,
            };
        }
        if scheme.eq_ignore_ascii_case("data") && kind == ImportMediaKind::MarkdownImage {
            return classify_inline_data(destination, limits);
        }
        return ImportMediaResolution::Blocked {
            reason: ImportMediaBlockedReason::UnsafeScheme,
        };
    }
    classify_local_destination(document_path, kind, destination, manifest_entries)
}

fn classify_inline_data(destination: &str, limits: MediaLimits) -> ImportMediaResolution {
    let Ok(data_url) = DataUrl::process(destination) else {
        return unsupported(ImportMediaUnsupportedReason::InvalidDataUrl);
    };
    let mime = if data_url.mime_type().matches("image", "png") {
        ImportInlineImageMime::Png
    } else if data_url.mime_type().matches("image", "jpeg") {
        ImportInlineImageMime::Jpeg
    } else {
        return unsupported(ImportMediaUnsupportedReason::InlineMime);
    };
    let mut decoded_bytes = Vec::new();
    let decoded = data_url.decode(|bytes| {
        let size = (decoded_bytes.len() as u64)
            .checked_add(bytes.len() as u64)
            .ok_or(())?;
        if size > limits.max_inline_decoded_bytes {
            return Err(());
        }
        decoded_bytes.try_reserve(bytes.len()).map_err(|_| ())?;
        decoded_bytes.extend_from_slice(bytes);
        Ok(())
    });
    let fragment = match decoded {
        Ok(fragment) => fragment,
        Err(DecodeError::InvalidBase64(_)) => {
            return unsupported(ImportMediaUnsupportedReason::InlineEncoding);
        }
        Err(DecodeError::WriteError(())) => {
            return unsupported(ImportMediaUnsupportedReason::InlineTooLarge);
        }
    };
    if fragment.is_some() {
        return unsupported(ImportMediaUnsupportedReason::InlineFragment);
    }
    let signature_matches = match mime {
        ImportInlineImageMime::Png => decoded_bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        ImportInlineImageMime::Jpeg => decoded_bytes.starts_with(b"\xff\xd8\xff"),
    };
    if !signature_matches {
        return unsupported(ImportMediaUnsupportedReason::InlineSignatureMismatch);
    }
    if let Err(reason) = validate_inline_image(&decoded_bytes, mime, limits) {
        return unsupported(reason);
    }
    let size = decoded_bytes.len() as u64;
    let decoded_sha256 = Sha256Digest::from_bytes(Sha256::digest(&decoded_bytes).into());
    ImportMediaResolution::InlineData {
        mime,
        decoded_size_bytes: size,
        decoded_sha256,
    }
}

fn validate_inline_image(
    bytes: &[u8],
    mime: ImportInlineImageMime,
    limits: MediaLimits,
) -> Result<(), ImportMediaUnsupportedReason> {
    let format = match mime {
        ImportInlineImageMime::Png => ImageFormat::Png,
        ImportInlineImageMime::Jpeg => ImageFormat::Jpeg,
    };
    let (width, height) = ImageReader::with_format(Cursor::new(bytes), format)
        .into_dimensions()
        .map_err(|_| ImportMediaUnsupportedReason::InlineImageDecode)?;
    if width == 0 || height == 0 {
        return Err(ImportMediaUnsupportedReason::InlineDimensions);
    }
    if width > limits.max_inline_image_width || height > limits.max_inline_image_height {
        return Err(ImportMediaUnsupportedReason::InlineDimensions);
    }
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or(ImportMediaUnsupportedReason::InlinePixelLimit)?;
    if pixels > limits.max_inline_image_pixels {
        return Err(ImportMediaUnsupportedReason::InlinePixelLimit);
    }

    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut decode_limits = Limits::default();
    decode_limits.max_image_width = Some(limits.max_inline_image_width);
    decode_limits.max_image_height = Some(limits.max_inline_image_height);
    decode_limits.max_alloc = Some(limits.max_inline_image_pixels.saturating_mul(16));
    reader.limits(decode_limits);
    reader
        .decode()
        .map(|_| ())
        .map_err(|_| ImportMediaUnsupportedReason::InlineImageDecode)
}

fn unsupported(reason: ImportMediaUnsupportedReason) -> ImportMediaResolution {
    ImportMediaResolution::Unsupported { reason }
}

fn classify_local_destination(
    document_path: &str,
    kind: ImportMediaKind,
    destination: &str,
    manifest_entries: &BTreeMap<&str, &ManifestEntry>,
) -> ImportMediaResolution {
    let decoded = match decode_local_path(destination) {
        Ok(decoded) => decoded,
        Err(reason) => return ImportMediaResolution::Blocked { reason },
    };
    let base = match kind {
        ImportMediaKind::MarkdownImage => document_path.rsplit_once('/').map(|(base, _)| base),
        ImportMediaKind::LegacyExcalidraw => None,
    };
    let resolved = match lexical_join(base, &decoded) {
        Ok(path) => path,
        Err(reason) => return ImportMediaResolution::Blocked { reason },
    };
    let Some(entry) = manifest_entries.get(resolved.as_str()) else {
        return ImportMediaResolution::Missing {
            attempted_relative_path: resolved,
        };
    };
    let expected_kind = match kind {
        ImportMediaKind::MarkdownImage => SourceKind::Asset,
        ImportMediaKind::LegacyExcalidraw => SourceKind::Drawing,
    };
    if entry.kind != expected_kind {
        return ImportMediaResolution::Blocked {
            reason: ImportMediaBlockedReason::ManifestKindMismatch,
        };
    }
    ImportMediaResolution::LocalManifest {
        relative_path: entry.relative_path.clone(),
        size_bytes: entry.size_bytes,
        sha256: entry.sha256,
    }
}

fn decode_local_path(destination: &str) -> Result<String, ImportMediaBlockedReason> {
    if destination.is_empty() {
        return Err(ImportMediaBlockedReason::InvalidRelativePath);
    }
    if destination.contains(['?', '#']) {
        return Err(ImportMediaBlockedReason::QueryOrFragment);
    }
    validate_percent_encoding(destination)?;
    let decoded = percent_decode_str(destination)
        .decode_utf8()
        .map_err(|_| ImportMediaBlockedReason::InvalidPercentEncodedUtf8)?
        .into_owned();
    if decoded.chars().any(is_control) {
        return Err(ImportMediaBlockedReason::ControlCharacter);
    }
    if is_windows_or_unc_path(&decoded) {
        return Err(ImportMediaBlockedReason::WindowsOrUncPath);
    }
    if decoded.contains('\\') {
        return Err(ImportMediaBlockedReason::Backslash);
    }
    if decoded.starts_with("//") {
        return Err(ImportMediaBlockedReason::ProtocolRelative);
    }
    if decoded.starts_with('/') {
        return Err(ImportMediaBlockedReason::AbsolutePath);
    }
    if uri_scheme(&decoded).is_some() {
        return Err(ImportMediaBlockedReason::UnsafeScheme);
    }
    if decoded.contains(['?', '#']) {
        return Err(ImportMediaBlockedReason::QueryOrFragment);
    }
    Ok(decoded)
}

fn validate_percent_encoding(value: &str) -> Result<(), ImportMediaBlockedReason> {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && (index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit())
        {
            return Err(ImportMediaBlockedReason::MalformedPercentEncoding);
        }
        index += if bytes[index] == b'%' { 3 } else { 1 };
    }
    Ok(())
}

fn lexical_join(base: Option<&str>, relative: &str) -> Result<String, ImportMediaBlockedReason> {
    let mut components = base
        .into_iter()
        .flat_map(|base| base.split('/'))
        .filter(|component| !component.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for component in relative.split('/') {
        match component {
            "" => return Err(ImportMediaBlockedReason::InvalidRelativePath),
            "." => {}
            ".." => {
                components
                    .pop()
                    .ok_or(ImportMediaBlockedReason::PathEscape)?;
            }
            value => components.push(value.to_owned()),
        }
    }
    if components.is_empty() {
        return Err(ImportMediaBlockedReason::InvalidRelativePath);
    }
    Ok(components.join("/"))
}

fn uri_scheme(value: &str) -> Option<&str> {
    let colon = value.find(':')?;
    let scheme = &value[..colon];
    let mut characters = scheme.chars();
    characters.next()?.is_ascii_alphabetic().then_some(())?;
    characters
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.'))
        .then_some(scheme)
}

fn is_windows_or_unc_path(value: &str) -> bool {
    value.starts_with("\\\\")
        || value.starts_with("\\?")
        || (value.as_bytes().get(1) == Some(&b':')
            && value
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphabetic))
}

fn is_control(character: char) -> bool {
    character <= '\u{1f}' || character == '\u{7f}'
}

fn is_legacy_excalidraw_destination(destination: &str) -> bool {
    const EXTENSION: &str = ".excalidraw";
    let Some(extension_start) = destination.len().checked_sub(EXTENSION.len()) else {
        return false;
    };
    destination
        .get(..6)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("draws/"))
        && destination
            .get(extension_start..)
            .is_some_and(|suffix| suffix.eq_ignore_ascii_case(EXTENSION))
}

fn push_resolution_diagnostic(
    document_path: &str,
    range: SourceRange,
    resolution: &ImportMediaResolution,
    diagnostic_limit: u64,
    remaining_diagnostics: &mut u64,
    diagnostics: &mut Vec<ImportDiagnostic>,
) -> Result<(), MediaCollectionError> {
    let diagnostic = match resolution {
        ImportMediaResolution::RemoteBlocked { .. } => Some((
            DiagnosticCode::RemoteMediaBlocked,
            "remote image was preserved but remains blocked by default",
            "Explicitly approve remote media loading after import",
        )),
        ImportMediaResolution::Missing { .. } => Some((
            DiagnosticCode::MissingMediaSource,
            "local media source is absent from the validated graph manifest",
            "Restore the source file or remove the reference before importing",
        )),
        ImportMediaResolution::Blocked { .. } => Some((
            DiagnosticCode::UnsafeMediaSource,
            "media source was preserved because its location is unsafe or unsupported",
            "Use a graph-relative source contained by the selected graph",
        )),
        ImportMediaResolution::Unsupported { .. } => Some((
            DiagnosticCode::UnsupportedInlineMedia,
            "inline image was preserved because its data is unsupported or invalid",
            "Use a bounded PNG or JPEG data URL, or a local graph asset",
        )),
        ImportMediaResolution::LocalManifest { .. } | ImportMediaResolution::InlineData { .. } => {
            None
        }
    };
    if let Some((code, message, remediation)) = diagnostic {
        if *remaining_diagnostics == 0 {
            return Err(MediaCollectionError::DiagnosticLimitExceeded {
                limit: diagnostic_limit,
            });
        }
        diagnostics.push(ImportDiagnostic::warning_at(
            code,
            document_path,
            range,
            message,
            Some(remediation.to_owned()),
        ));
        *remaining_diagnostics -= 1;
    }
    Ok(())
}

struct SourceCursor<'source> {
    source: &'source str,
    relative_path: &'source str,
    byte_offset: usize,
    line: u64,
    column: u64,
}

impl<'source> SourceCursor<'source> {
    const fn new(source: &'source str, relative_path: &'source str) -> Self {
        Self {
            source,
            relative_path,
            byte_offset: 0,
            line: 1,
            column: 1,
        }
    }

    fn range(&mut self, range: Range<usize>) -> Result<SourceRange, MediaCollectionError> {
        let start = self.position(range.start)?;
        let end = self.position(range.end)?;
        Ok(SourceRange { start, end })
    }

    fn position(&mut self, byte_offset: usize) -> Result<SourcePosition, MediaCollectionError> {
        if byte_offset < self.byte_offset
            || byte_offset > self.source.len()
            || !self.source.is_char_boundary(byte_offset)
        {
            return Err(invalid_mapping(self.relative_path));
        }
        for character in self.source[self.byte_offset..byte_offset].chars() {
            if character == '\n' {
                self.line += 1;
                self.column = 1;
            } else {
                self.column += 1;
            }
        }
        self.byte_offset = byte_offset;
        Ok(SourcePosition {
            line: self.line,
            column: self.column,
            byte_offset: byte_offset as u64,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_owner_has_an_independent_markdown_parser_state() {
        assert!(parse_media_tokens("```md\n![hidden](../assets/hidden.png)").is_empty());
        let markdown = "![visible](../assets/visible.png)";
        let sibling = parse_media_tokens(markdown);
        assert_eq!(sibling.len(), 1);
        assert_eq!(sibling[0].range, 0..markdown.len());
    }

    #[test]
    fn parser_offsets_cover_whole_tokens_and_ignore_code_and_escapes() {
        let markdown =
            "`![code](a.png)` \\![escaped](b.png) ![real](c.png \"title\") [[draws/d.excalidraw]]";
        let tokens = parse_media_tokens(markdown);
        assert_eq!(tokens.len(), 2);
        assert_eq!(
            &markdown[tokens[0].range.clone()],
            "![real](c.png \"title\")"
        );
        assert_eq!(&markdown[tokens[1].range.clone()], "[[draws/d.excalidraw]]");
    }
}
