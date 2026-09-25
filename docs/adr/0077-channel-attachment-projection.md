# ADR 0077: Project Channel Attachments into Typed Agent Messages

## Status

Accepted

- Date: 2026-09-25
- Owner: framework channel adapter

## Context

`InboundMessage` already carries ordered binary attachments, but
`AgentChannelHandler` built a text-only `TurnRequest`. Custom or future media
channels could therefore make a successful model call while silently losing
the attachments. The existing `Message::user_multimodal` and
`TurnRequest::from_message` contracts already carry text, image URLs, and
base64 files without a new channel or Turn authority.

## Options Considered

1. Keep channels text-only and reject every attachment. This loses supported
   image and file input despite the existing typed Agent contract.
2. Add a new channel-specific multimodal request and media metadata contract.
   This duplicates `ContentPart` and expands the public protocol without a
   need for the current attachment kinds.
3. Convert supported attachments at the channel adapter into the existing
   typed message, and fail before model admission for unrepresentable input.
   Chosen.

## Decision

- Text-only inbound messages keep the existing `TurnInput::Text` path.
- An inbound message with attachments becomes one typed user message. Non-empty
  text is the first part; attachments follow their received order. A message
  containing only attachments does not invent a text part.
- Image MIME is determined from supported PNG, JPEG, GIF, and WebP byte
  signatures, not a possibly misleading filename. Unknown image bytes fail
  before the Turn driver invokes the model.
- Files retain exact bytes in base64 `ContentPart::File`; an absent or empty
  filename receives a deterministic positional name. Audio and video have no
  core typed representation and fail explicitly before model invocation.
- `ImageUrl` cannot retain an image attachment's original filename, and
  `ContentPart::File` requires a concrete name rather than the transport's
  optional `None`. This decision preserves bytes and part order, not every
  transport metadata field; those metadata limits must remain visible to
  consumers rather than being called fully lossless.
- The adapter does not claim that every provider can read every typed file.
  Provider protocol translation and model modality checks retain their own
  existing responsibility; this decision closes the channel projection loss.

## Consequences

The framework channel adapter has one typed input projection shared by
`handle`, `handle_stream`, and `drive_turn_with_sink`, without a second Turn
state machine. Invalid media no longer produces an apparently successful
text-only reply. QQ and Feishu transport media acquisition remains outside
this adapter and keeps their existing capability declarations. Provider
specific file-read support must be assessed at the provider boundary.

## Verification

Focused channel tests cover a real synchronous model request with binary file
bytes, attachment-only streaming, ordered text/file/image/file content, four
image signatures, and pre-model rejection of audio, video, and unknown images.
