package com.echoagent.sdk;

import org.junit.jupiter.api.Test;

import java.nio.charset.StandardCharsets;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

class Utf8SupportTest {
    @Test
    void splitUtf8ChunksKeepsCodePointsWithinByteCap() {
        assertEquals(List.of("\u4e2d", "\u6587ab"),
                Utf8Support.splitUtf8Chunks("\u4e2d\u6587ab", 5));
        assertEquals(List.of("\ud83d\ude00"), Utf8Support.splitUtf8Chunks("\ud83d\ude00", 1));
    }

    @Test
    void decoderPreservesSplitScalarsAndFlushesIncompleteSuffix() {
        var decoder = new IncrementalUtf8Decoder(16);
        assertTrue(decoder.push(new byte[] {(byte) 0xe4, (byte) 0xb8}).isEmpty());
        assertEquals(List.of("\u4e2d"), decoder.push(new byte[] {(byte) 0xad}));
        assertTrue(decoder.finish().isEmpty());

        var incomplete = new IncrementalUtf8Decoder(16);
        assertTrue(incomplete.push(new byte[] {(byte) 0xf0, (byte) 0x9f}).isEmpty());
        assertEquals("\ufffd", incomplete.finish().orElseThrow());
        assertTrue(incomplete.finish().isEmpty());
    }

    @Test
    void decoderReplacesMalformedBytes() {
        var decoder = new IncrementalUtf8Decoder(16);
        assertEquals(List.of("a\ufffd"), decoder.push(new byte[] {'a', (byte) 0xff}));

        var malformedScalar = new IncrementalUtf8Decoder(16);
        assertEquals(List.of("\ufffd\ufffd\ufffd"),
                malformedScalar.push(new byte[] {(byte) 0xed, (byte) 0xa0, (byte) 0x80}));

        var invalidContinuation = new IncrementalUtf8Decoder(16);
        assertEquals(List.of("\ufffd("),
                invalidContinuation.push(new byte[] {(byte) 0xe2, (byte) 0x28}));

        var invalidTailBeforeEnd = new IncrementalUtf8Decoder(16);
        assertEquals(List.of("\ufffd\ufffd"),
                invalidTailBeforeEnd.push(new byte[] {(byte) 0xf0, (byte) 0x90, (byte) 0xff}));
    }

    @Test
    void jsonHelpersHandleFencesAndTrailingCommasOutsideStrings() {
        String fenced = "prefix\n```json\n{\"key\": \"value\"}\n```\nsuffix";
        assertEquals("{\"key\": \"value\"}", Utf8Support.extractJsonFromMarkdown(fenced));
        assertEquals("{\"a\": 1, \"text\": \"keep ,} and ,] literal\"}",
                Utf8Support.cleanJson("{\"a\": 1, \"text\": \"keep ,} and ,] literal\",}"));
        assertEquals("{'a': 'don\\'t'}", Utf8Support.cleanJson("{'a': 'don\\'t'}"));
    }

    @Test
    void decoderUsesUtf8ByteCapForReturnedChunks() {
        var decoder = new IncrementalUtf8Decoder(5);
        assertEquals(List.of("\u4e2d", "\u6587ab"),
                decoder.push("\u4e2d\u6587ab".getBytes(StandardCharsets.UTF_8)));
    }
}
