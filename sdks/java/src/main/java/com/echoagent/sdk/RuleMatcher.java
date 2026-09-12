package com.echoagent.sdk;

import java.util.List;

/** Permission matcher values and pure matching helpers. */
public sealed interface RuleMatcher permits RuleMatcher.Tool, RuleMatcher.Pattern, RuleMatcher.Permission, RuleMatcher.All {
    static RuleMatcher parse(String value) {
        if ("*".equals(value) || "all".equals(value)) return new All();
        if (value != null && value.startsWith("tool:")) {
            var name = value.substring(5);
            if (name.isEmpty()) throw new IllegalArgumentException("tool permission matcher requires a name");
            return new Tool(name);
        }
        if (value != null && value.startsWith("pattern:")) {
            var pattern = value.substring(8);
            if (pattern.isEmpty()) throw new IllegalArgumentException("pattern permission matcher cannot be empty");
            return new Pattern(pattern);
        }
        var flag = value != null && value.startsWith("perm:") ? value.substring(5)
                : value != null && value.startsWith("permission:") ? value.substring(11) : null;
        if (flag != null) {
            return switch (flag) {
                case "read" -> new Permission(ToolPermission.READ);
                case "write" -> new Permission(ToolPermission.WRITE);
                case "network" -> new Permission(ToolPermission.NETWORK);
                case "execute" -> new Permission(ToolPermission.EXECUTE);
                case "sensitive" -> new Permission(ToolPermission.SENSITIVE);
                default -> throw new IllegalArgumentException("unknown permission matcher: " + flag);
            };
        }
        throw new IllegalArgumentException("unsupported permission matcher: " + value);
    }

    String display();
    boolean matches(String toolName, List<ToolPermission> permissions);
    boolean matchesMatcherStr(String value);

    record Tool(String name) implements RuleMatcher {
        public Tool { if (name == null || name.isEmpty()) throw new IllegalArgumentException("tool name is required"); }
        public String display() { return "tool:" + name; }
        public boolean matches(String toolName, List<ToolPermission> permissions) { return name.equals(toolName); }
        public boolean matchesMatcherStr(String value) { return name.equals(value); }
    }

    record Pattern(String pattern) implements RuleMatcher {
        public Pattern { if (pattern == null || pattern.isEmpty()) throw new IllegalArgumentException("pattern is required"); }
        public String display() { return "pattern:" + pattern; }
        public boolean matches(String toolName, List<ToolPermission> permissions) {
            if (toolName.equals(pattern)) return true;
            if (globMatches(pattern, toolName)) return true;
            if (pattern.endsWith("*)") && toolName.startsWith(pattern.substring(0, pattern.length() - 2))) return true;
            return toolName.startsWith(pattern) && toolName.length() > pattern.length() && toolName.charAt(pattern.length()) == '(';
        }
        public boolean matchesMatcherStr(String value) { return pattern.equals(value); }

        private static boolean globMatches(String pattern, String value) {
            int p = 0;
            int v = 0;
            int star = -1;
            int mark = -1;
            while (v < value.length()) {
                if (p < pattern.length() && (pattern.charAt(p) == '?' || pattern.charAt(p) == value.charAt(v))) {
                    p++;
                    v++;
                } else if (p < pattern.length() && pattern.charAt(p) == '*') {
                    star = p++;
                    mark = v;
                } else if (star != -1) {
                    p = star + 1;
                    v = ++mark;
                } else {
                    return false;
                }
            }
            while (p < pattern.length() && pattern.charAt(p) == '*') p++;
            return p == pattern.length();
        }
    }

    record Permission(ToolPermission permission) implements RuleMatcher {
        public String display() { return "permission:" + permission; }
        public boolean matches(String toolName, List<ToolPermission> permissions) { return permissions.contains(permission); }
        public boolean matchesMatcherStr(String value) { return false; }
    }

    record All() implements RuleMatcher {
        public String display() { return "all"; }
        public boolean matches(String toolName, List<ToolPermission> permissions) { return true; }
        public boolean matchesMatcherStr(String value) { return "*".equals(value) || "all".equals(value); }
    }
}
