package com.echoagent.sdk;

/** Pagination metadata projection without owning collection state. */
public record PageInfo(String nextCursor, boolean truncated, boolean totalKnown, Long total, long returned) {
    public ToolResult applyTo(ToolResult result) {
        var updated = result.withTruncated(result.toJson().path("truncated").asBoolean(false) || truncated)
                .withMeta("page.truncated", Boolean.toString(truncated))
                .withMeta("page.total_known", Boolean.toString(totalKnown))
                .withMeta("page.returned", Long.toString(returned));
        if (total != null) updated = updated.withMeta("page.total", Long.toString(total));
        if (nextCursor != null) {
            updated = updated.withMeta("page.next_cursor", nextCursor);
            String continuation = "{\"next_cursor\":\"" + nextCursor + "\",\"returned\":" + returned
                    + ",\"total\":" + (total == null ? "null" : total)
                    + ",\"total_known\":" + totalKnown + ",\"truncated\":true}";
            updated = updated.withOutput(updated.toJson().path("output").asText("") + "\n[page]" + continuation);
        }
        return updated;
    }
}
