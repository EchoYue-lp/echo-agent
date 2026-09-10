package com.echoagent.sdk;

import java.io.ByteArrayOutputStream;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.Objects;
import java.util.Optional;

/**
 * Incrementally decodes UTF-8 byte streams whose read boundaries may split a scalar.
 * Invalid sequences become U+FFFD; an incomplete suffix stays pending until the next
 * push or {@link #finish()} call.
 */
public final class IncrementalUtf8Decoder {
    private static final int DEFAULT_MAX_CHUNK_BYTES = 16 * 1024;

    private final int maxChunkBytes;
    private final ByteArrayOutputStream pending = new ByteArrayOutputStream();

    public IncrementalUtf8Decoder() {
        this(DEFAULT_MAX_CHUNK_BYTES);
    }

    public IncrementalUtf8Decoder(int maxChunkBytes) {
        this.maxChunkBytes = Math.max(1, maxChunkBytes);
    }

    /** Decodes one byte slice and returns byte-capped, code-point-safe chunks. */
    public List<String> push(byte[] bytes) {
        Objects.requireNonNull(bytes, "bytes");
        pending.writeBytes(bytes);

        byte[] inputBytes = pending.toByteArray();
        var output = new StringBuilder();
        int index = 0;
        while (index < inputBytes.length) {
            int first = inputBytes[index] & 0xff;
            if (first <= 0x7f) {
                output.append((char) first);
                index++;
                continue;
            }

            int width = utf8Width(first);
            if (width == 0) {
                output.append('\ufffd');
                index++;
                continue;
            }
            if (inputBytes.length - index < 2) break;

            int second = inputBytes[index + 1] & 0xff;
            if (!validSecondByte(first, second)) {
                output.append('\ufffd');
                index++;
                continue;
            }
            int consumed = 2;
            while (consumed < width
                    && index + consumed < inputBytes.length
                    && isContinuation(inputBytes[index + consumed] & 0xff)) {
                consumed++;
            }
            if (consumed < width) {
                if (index + consumed >= inputBytes.length) break;
                // Match Rust's from_utf8 error span: consume the leading byte and any
                // already-valid continuation prefix, leaving the offending byte for the
                // next iteration so it receives its own replacement if necessary.
                output.append('\ufffd');
                index += consumed;
                continue;
            }

            int codePoint = decodeCodePoint(inputBytes, index, width);
            output.appendCodePoint(codePoint);
            index += width;
        }

        pending.reset();
        if (index < inputBytes.length) pending.write(inputBytes, index, inputBytes.length - index);
        return Utf8Support.splitUtf8Chunks(output.toString(), maxChunkBytes);
    }

    /** Flushes a pending incomplete suffix as U+FFFD, if one exists. */
    public Optional<String> finish() {
        if (pending.size() == 0) return Optional.empty();
        String output = new String(pending.toByteArray(), StandardCharsets.UTF_8);
        pending.reset();
        return Optional.of(output);
    }

    private static int utf8Width(int first) {
        if (first >= 0xc2 && first <= 0xdf) return 2;
        if (first >= 0xe0 && first <= 0xef) return 3;
        if (first >= 0xf0 && first <= 0xf4) return 4;
        return 0;
    }

    private static boolean isContinuation(int value) {
        return value >= 0x80 && value <= 0xbf;
    }

    private static boolean validSecondByte(int first, int second) {
        if (!isContinuation(second)) return false;
        if (first == 0xe0) return second >= 0xa0;
        if (first == 0xed) return second <= 0x9f;
        if (first == 0xf0) return second >= 0x90;
        if (first == 0xf4) return second <= 0x8f;
        return true;
    }

    private static int decodeCodePoint(byte[] bytes, int index, int width) {
        int codePoint;
        if (width == 2) {
            codePoint = ((bytes[index] & 0x1f) << 6) | (bytes[index + 1] & 0x3f);
        } else if (width == 3) {
            codePoint = ((bytes[index] & 0x0f) << 12)
                    | ((bytes[index + 1] & 0x3f) << 6)
                    | (bytes[index + 2] & 0x3f);
        } else {
            codePoint = ((bytes[index] & 0x07) << 18)
                    | ((bytes[index + 1] & 0x3f) << 12)
                    | ((bytes[index + 2] & 0x3f) << 6)
                    | (bytes[index + 3] & 0x3f);
        }
        return codePoint;
    }
}
