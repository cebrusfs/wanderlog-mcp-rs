// License texts of every crate compiled into the wanderlog-mcp binary, for desktop bundles.
// Fails instead of guessing: a crate without a declared license or license file stops packaging.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { basename, dirname, join } from "node:path";

// Crate archives that omit their license text. Each override is the unmodified upstream file,
// pinned by crate version and SHA-256 (see third-party-licenses/README.md next to this script).
const OVERRIDES = new Map([
  ["rmcp 3.5.1", "rmcp-3.5.1.txt"],
  ["rmcp-macros 3.5.1", "rmcp-3.5.1.txt"],
]);
const OVERRIDE_SHA256 = new Map([
  ["rmcp-3.5.1.txt", "0382b0057770ca05e9c350a50aa3b1c1fea84da0bc81d723bf00b9aa841be58a"],
]);
const LICENSE_FILE = /^(LICEN[CS]E|COPYING|NOTICE)/i;

export function thirdPartyNotices(repo, target, root = "wanderlog-mcp") {
  const result = spawnSync(
    "cargo",
    ["metadata", "--locked", "--format-version", "1", "--filter-platform", target],
    { cwd: repo, encoding: "utf8", maxBuffer: 256 * 1024 * 1024 },
  );
  if (result.error) throw result.error;
  assert.equal(result.status, 0, `cargo metadata failed: ${result.stderr}`);
  const metadata = JSON.parse(result.stdout);
  const packages = new Map(metadata.packages.map((p) => [p.id, p]));
  const nodes = new Map(metadata.resolve.nodes.map((n) => [n.id, n]));

  // Normal dependencies reachable from the binary; dev-only and build-only crates are not shipped.
  const seen = new Set();
  const pending = metadata.packages.filter((p) => p.name === root && p.source === null).map((p) => p.id);
  assert.equal(pending.length, 1, `workspace package ${root} not found`);
  while (pending.length) {
    const id = pending.pop();
    if (seen.has(id)) continue;
    seen.add(id);
    for (const dep of nodes.get(id).deps) {
      if (dep.dep_kinds.some((kind) => kind.kind === null)) pending.push(dep.pkg);
    }
  }

  const external = [...seen]
    .map((id) => packages.get(id))
    .filter((p) => p.source !== null)
    .sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));
  let text = `Third-party software in ${root} (${target})\n`;
  for (const p of external) {
    assert(p.license, `${p.name} ${p.version} declares no SPDX license`);
    const override = OVERRIDES.get(`${p.name} ${p.version}`);
    let files;
    if (override) {
      const path = join(repo, "scripts", "third-party-licenses", override);
      const digest = createHash("sha256").update(readFileSync(path)).digest("hex");
      assert.equal(digest, OVERRIDE_SHA256.get(override), `${override} differs from the pinned upstream text`);
      files = [path];
    } else {
      const dir = dirname(p.manifest_path);
      files = readdirSync(dir)
        .filter((name) => LICENSE_FILE.test(name) && statSync(join(dir, name)).isFile())
        .sort()
        .map((name) => join(dir, name));
    }
    assert(files.length, `${p.name} ${p.version} ships no license text; add a pinned override`);
    text += `\n${"=".repeat(72)}\n${p.name} ${p.version} (${p.license})\n${p.repository ?? ""}\n`;
    for (const file of files) text += `\n--- ${basename(file)} ---\n${readFileSync(file, "utf8")}`;
  }
  return { text, count: external.length };
}
