package com.echoagent.sdk;

import java.net.URI;
import java.util.List;
import java.util.Map;

/** Hook action configuration values without executing hooks. */
public sealed interface HookAction
        permits HookAction.Command, HookAction.Prompt, HookAction.Permission,
        HookAction.Http, HookAction.McpTool, HookAction.Subagent, HookAction.ActivateSkill {
    String kind();
    void validate();

    record Command(String command, String shell, long timeout) implements HookAction {
        public String kind() { return "command"; }
        public void validate() {
            if (command == null || command.isEmpty()) throw new IllegalArgumentException("Command hook has empty command string");
            if (command.codePointCount(0, command.length()) > 32 * 1024) throw new IllegalArgumentException("Command hook exceeds max length");
            if (timeout > 3600) throw new IllegalArgumentException("Command hook timeout exceeds maximum");
        }
    }
    record Prompt(String prompt) implements HookAction {
        public String kind() { return "prompt"; }
        public void validate() { if (prompt == null || prompt.isEmpty()) throw new IllegalArgumentException("Prompt hook has empty prompt string"); }
    }
    record Permission(String decision, String reason, List<String> suggestions) implements HookAction {
        public Permission { suggestions = List.copyOf(suggestions); }
        public String kind() { return "permission"; }
        public void validate() { if (!List.of("allow", "deny", "ask").contains(decision)) throw new IllegalArgumentException("Permission hook has invalid decision"); }
    }
    record Http(String url, String method, Map<String, String> headers, long timeout) implements HookAction {
        public Http { headers = headers == null ? null : Map.copyOf(headers); }
        public String kind() { return "http"; }
        public void validate() {
            if (url == null || url.isEmpty()) throw new IllegalArgumentException("Http hook has empty url");
            URI parsed = URI.create(url);
            boolean local = "localhost".equals(parsed.getHost()) || "127.0.0.1".equals(parsed.getHost()) || "::1".equals(parsed.getHost());
            if (!"https".equals(parsed.getScheme()) && !("http".equals(parsed.getScheme()) && local)) throw new IllegalArgumentException("Http hook must use https unless it targets a local address");
            if (timeout > 3600) throw new IllegalArgumentException("Http hook timeout exceeds maximum");
        }
    }
    record McpTool(String server, String tool, Object arguments, long timeout) implements HookAction {
        public String kind() { return "mcp_tool"; }
        public void validate() {
            if (server == null || server.isEmpty()) throw new IllegalArgumentException("McpTool hook has empty server name");
            if (tool == null || tool.isEmpty()) throw new IllegalArgumentException("McpTool hook has empty tool name");
            if (timeout > 3600) throw new IllegalArgumentException("McpTool hook timeout exceeds maximum");
        }
    }
    record Subagent(String name, String task, long timeout) implements HookAction {
        public String kind() { return "subagent"; }
        public void validate() {
            if (name == null || name.isEmpty()) throw new IllegalArgumentException("Subagent hook has empty subagent name");
            if (timeout > 3600) throw new IllegalArgumentException("Subagent hook timeout exceeds maximum");
        }
    }
    record ActivateSkill(String skill, String reason) implements HookAction {
        public String kind() { return "activate_skill"; }
        public void validate() { if (skill == null || skill.isEmpty()) throw new IllegalArgumentException("ActivateSkill hook has empty skill name"); }
    }
}
