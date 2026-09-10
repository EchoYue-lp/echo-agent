package com.echoagent.sdk;

import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.Objects;

/** Language-local helpers that do not participate in ACP or Host protocol traffic. */
public final class Utf8Support {
    private Utf8Support() {}

    /**
     * Splits text at Unicode code point boundaries while keeping each chunk within the
     * requested UTF-8 byte cap. A single code point may exceed a cap smaller than its
     * encoded width, as it cannot be split without producing invalid UTF-8.
     */
    public static List<String> splitUtf8Chunks(String text, int maxChunkBytes) {
        Objects.requireNonNull(text, "text");
        if (text.isEmpty()) return List.of();

        int cap = Math.max(1, maxChunkBytes);
        if (text.getBytes(StandardCharsets.UTF_8).length <= cap) return List.of(text);

        var chunks = new ArrayList<String>();
        var current = new StringBuilder();
        int currentBytes = 0;
        for (int offset = 0; offset < text.length();) {
            int codePoint = text.codePointAt(offset);
            int codePointChars = Character.charCount(codePoint);
            int codePointBytes = utf8Width(codePoint);
            if (currentBytes + codePointBytes > cap && current.length() > 0) {
                chunks.add(current.toString());
                current.setLength(0);
                currentBytes = 0;
            }
            current.appendCodePoint(codePoint);
            currentBytes += codePointBytes;
            offset += codePointChars;
        }
        if (current.length() > 0) chunks.add(current.toString());
        return List.copyOf(chunks);
    }

    /** Extracts JSON from a fenced markdown block or returns trimmed bare text. */
    public static String extractJsonFromMarkdown(String content) {
        Objects.requireNonNull(content, "content");
        int languageStart = content.indexOf("```json");
        if (languageStart >= 0) {
            int bodyStart = languageStart + "```json".length();
            int end = content.indexOf("```", bodyStart);
            if (end >= 0) return stripUnicodeWhitespace(content.substring(bodyStart, end));
        }

        int fenceStart = content.indexOf("```");
        if (fenceStart >= 0) {
            int bodyStart = fenceStart + "```".length();
            int end = content.indexOf("```", bodyStart);
            if (end >= 0) return stripUnicodeWhitespace(content.substring(bodyStart, end));
        }
        return stripUnicodeWhitespace(content);
    }

    /** Removes trailing commas before a closing object/array delimiter outside strings. */
    public static String cleanJson(String json) {
        Objects.requireNonNull(json, "json");
        var cleaned = new StringBuilder(json.length());
        boolean inString = false;
        boolean escaped = false;
        for (int index = 0; index < json.length();) {
            char character = json.charAt(index);
            if (inString) {
                cleaned.append(character);
                if (escaped) {
                    escaped = false;
                } else if (character == '\\') {
                    escaped = true;
                } else if (character == '"') {
                    inString = false;
                }
                index++;
                continue;
            }

            if (character == '"') {
                inString = true;
                cleaned.append(character);
                index++;
                continue;
            }
            if (character == ',') {
                int lookahead = index + 1;
                while (lookahead < json.length()) {
                    int next = json.codePointAt(lookahead);
                    if (!isWhitespace(next)) break;
                    lookahead += Character.charCount(next);
                }
                if (lookahead < json.length()) {
                    char next = json.charAt(lookahead);
                    if (next == '}' || next == ']') {
                        index++;
                        continue;
                    }
                }
            }
            cleaned.append(character);
            index++;
        }
        return cleaned.toString();
    }

    private static int utf8Width(int codePoint) {
        if (codePoint <= 0x7f) return 1;
        if (codePoint <= 0x7ff) return 2;
        if (codePoint <= 0xffff) return 3;
        return 4;
    }

    private static boolean isWhitespace(int codePoint) {
        return Character.isWhitespace(codePoint) || Character.isSpaceChar(codePoint);
    }

    private static String stripUnicodeWhitespace(String value) {
        int start = 0;
        int end = value.length();
        while (start < end) {
            int codePoint = value.codePointAt(start);
            if (!isWhitespace(codePoint)) break;
            start += Character.charCount(codePoint);
        }
        while (end > start) {
            int codePoint = value.codePointBefore(end);
            if (!isWhitespace(codePoint)) break;
            end -= Character.charCount(codePoint);
        }
        return value.substring(start, end);
    }
}
