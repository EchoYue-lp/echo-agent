#!/usr/bin/env -S npx tsx
/**
 * Summarize project dependencies from package.json and/or Cargo.toml.
 *
 * Usage: npx tsx dep_summary.ts [directory]
 *
 * Outputs JSON with dependency counts and categorized listings.
 * Supports: npm (package.json), Cargo (Cargo.toml), pip (requirements.txt).
 */

import { readFileSync, existsSync } from "fs";
import { join, resolve } from "path";

interface DepInfo {
  manager: string;
  file: string;
  direct: string[];
  dev: string[];
}

function parsePackageJson(dir: string): DepInfo | null {
  const file = join(dir, "package.json");
  if (!existsSync(file)) return null;

  try {
    const pkg = JSON.parse(readFileSync(file, "utf-8"));
    return {
      manager: "npm",
      file: "package.json",
      direct: Object.keys(pkg.dependencies ?? {}),
      dev: Object.keys(pkg.devDependencies ?? {}),
    };
  } catch {
    return null;
  }
}

function parseCargoToml(dir: string): DepInfo | null {
  const file = join(dir, "Cargo.toml");
  if (!existsSync(file)) return null;

  try {
    const content = readFileSync(file, "utf-8");
    const direct: string[] = [];
    const dev: string[] = [];

    let inDeps = false;
    let inDevDeps = false;

    for (const line of content.split("\n")) {
      const trimmed = line.trim();

      if (trimmed === "[dependencies]") {
        inDeps = true;
        inDevDeps = false;
        continue;
      }
      if (trimmed === "[dev-dependencies]") {
        inDeps = false;
        inDevDeps = true;
        continue;
      }
      if (trimmed.startsWith("[")) {
        inDeps = false;
        inDevDeps = false;
        continue;
      }

      const match = trimmed.match(/^([a-zA-Z0-9_-]+)\s*=/);
      if (match) {
        if (inDeps) direct.push(match[1]);
        if (inDevDeps) dev.push(match[1]);
      }
    }

    return { manager: "cargo", file: "Cargo.toml", direct, dev };
  } catch {
    return null;
  }
}

function parseRequirementsTxt(dir: string): DepInfo | null {
  const file = join(dir, "requirements.txt");
  if (!existsSync(file)) return null;

  try {
    const content = readFileSync(file, "utf-8");
    const deps = content
      .split("\n")
      .map((l) => l.trim())
      .filter((l) => l && !l.startsWith("#") && !l.startsWith("-"))
      .map((l) => l.split(/[>=<!~]/)[0].trim());

    return { manager: "pip", file: "requirements.txt", direct: deps, dev: [] };
  } catch {
    return null;
  }
}

function main() {
  const dir = resolve(process.argv[2] ?? ".");

  const parsers = [parsePackageJson, parseCargoToml, parseRequirementsTxt];
  const results = parsers.map((p) => p(dir)).filter(Boolean) as DepInfo[];

  if (results.length === 0) {
    console.log(
      JSON.stringify({
        directory: dir,
        error: "No recognized dependency files found",
        checked: ["package.json", "Cargo.toml", "requirements.txt"],
      })
    );
    process.exit(0);
  }

  const output = {
    directory: dir,
    managers: results.map((r) => ({
      manager: r.manager,
      file: r.file,
      direct_count: r.direct.length,
      dev_count: r.dev.length,
      total: r.direct.length + r.dev.length,
      direct: r.direct.sort(),
      dev: r.dev.sort(),
    })),
    total_dependencies: results.reduce(
      (sum, r) => sum + r.direct.length + r.dev.length,
      0
    ),
  };

  console.log(JSON.stringify(output, null, 2));
}

main();
