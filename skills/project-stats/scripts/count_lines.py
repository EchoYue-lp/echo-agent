#!/usr/bin/env python3
"""Count lines of code by language in a directory tree.

Usage: python3 count_lines.py [directory]

Outputs a JSON summary with per-language file counts and line counts.
Skips common non-source directories (.git, node_modules, target, etc.).
"""

import json
import os
import sys
from collections import defaultdict

EXTENSION_MAP = {
    ".py": "Python",
    ".rs": "Rust",
    ".js": "JavaScript",
    ".ts": "TypeScript",
    ".tsx": "TypeScript (JSX)",
    ".jsx": "JavaScript (JSX)",
    ".java": "Java",
    ".go": "Go",
    ".c": "C",
    ".cpp": "C++",
    ".h": "C/C++ Header",
    ".cs": "C#",
    ".rb": "Ruby",
    ".php": "PHP",
    ".swift": "Swift",
    ".kt": "Kotlin",
    ".scala": "Scala",
    ".sh": "Shell",
    ".bash": "Shell",
    ".zsh": "Shell",
    ".sql": "SQL",
    ".html": "HTML",
    ".css": "CSS",
    ".scss": "SCSS",
    ".less": "LESS",
    ".md": "Markdown",
    ".yaml": "YAML",
    ".yml": "YAML",
    ".toml": "TOML",
    ".json": "JSON",
    ".xml": "XML",
}

SKIP_DIRS = {
    ".git", "node_modules", "target", "__pycache__", ".venv", "venv",
    "dist", "build", ".next", ".nuxt", "vendor", ".cargo",
}


def count_lines_in_file(filepath):
    try:
        with open(filepath, "r", encoding="utf-8", errors="ignore") as f:
            lines = f.readlines()
            total = len(lines)
            blank = sum(1 for line in lines if not line.strip())
            return total, total - blank
    except (OSError, UnicodeDecodeError):
        return 0, 0


def scan_directory(root):
    stats = defaultdict(lambda: {"files": 0, "total_lines": 0, "code_lines": 0})

    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]

        for filename in filenames:
            ext = os.path.splitext(filename)[1].lower()
            lang = EXTENSION_MAP.get(ext)
            if not lang:
                continue

            filepath = os.path.join(dirpath, filename)
            total, code = count_lines_in_file(filepath)

            stats[lang]["files"] += 1
            stats[lang]["total_lines"] += total
            stats[lang]["code_lines"] += code

    return dict(stats)


def main():
    directory = sys.argv[1] if len(sys.argv) > 1 else "."
    directory = os.path.abspath(directory)

    if not os.path.isdir(directory):
        print(json.dumps({"error": f"Not a directory: {directory}"}))
        sys.exit(1)

    stats = scan_directory(directory)

    total_files = sum(s["files"] for s in stats.values())
    total_lines = sum(s["total_lines"] for s in stats.values())
    total_code = sum(s["code_lines"] for s in stats.values())

    sorted_langs = sorted(stats.items(), key=lambda x: x[1]["code_lines"], reverse=True)

    result = {
        "directory": directory,
        "summary": {
            "total_files": total_files,
            "total_lines": total_lines,
            "code_lines": total_code,
            "blank_lines": total_lines - total_code,
            "languages_detected": len(stats),
        },
        "languages": [
            {
                "language": lang,
                "files": data["files"],
                "total_lines": data["total_lines"],
                "code_lines": data["code_lines"],
                "percentage": round(data["code_lines"] / total_code * 100, 1) if total_code else 0,
            }
            for lang, data in sorted_langs
        ],
    }

    print(json.dumps(result, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
