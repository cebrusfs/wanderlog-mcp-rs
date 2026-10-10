import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { constants, accessSync, chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { getMcpConfigForManifest, packExtension, unpackExtension } from "@anthropic-ai/mcpb";
import Ajv2020 from "ajv/dist/2020.js";
import { thirdPartyNotices } from "./third-party-notices.mjs";

// Cargo owns package identity; both client adapters wrap the same release binary.
const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const metadata = JSON.parse(run("cargo", ["metadata", "--no-deps", "--format-version", "1", "--locked"]));
const { name, version, description } = metadata.packages.find((p) => p.name === "wanderlog-mcp");
assert.equal(process.platform, "darwin", "Desktop packaging currently supports macOS only");
const cpu = { arm64: "aarch64", x64: "x86_64" }[process.arch];
assert(cpu, `Unsupported macOS architecture: ${process.arch}`);
const target = `${cpu}-apple-darwin`;
const label = `${name}-${version}-macos-${process.arch}`;
const author = { name: "Wanderlog MCP contributors" };
const entryPoint = `server/${name}`;
const args = ["serve"];

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { cwd: repo, encoding: "utf8", timeout: 120_000, ...options });
  if (result.error) throw result.error;
  assert.equal(result.status, 0, `${command} failed: ${result.stderr ?? ""}`);
  return result.stdout?.trim();
}

function json(path, value) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`);
}

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

async function validatePortable(document) {
  const response = await fetch(document.$schema, { signal: AbortSignal.timeout(15_000) });
  assert(response.ok, `Cannot fetch schema: ${document.$schema}`);
  const validator = new Ajv2020({ strict: false }).compile(await response.json());
  assert(validator(document), JSON.stringify(validator.errors));
}

// Only handshake and discovery: no credentials, Keychain access, or Wanderlog requests.
async function smoke(command, args, cwd, expectedVersion) {
  accessSync(command, constants.X_OK);
  const child = spawn(command, args, {
    cwd,
    env: { PATH: "/usr/bin:/bin", WANDERLOG_COOKIE: "package-smoke-not-a-session" },
    stdio: ["pipe", "pipe", "pipe"],
  });
  const lines = createInterface({ input: child.stdout });
  let diagnostics = "";
  child.stderr.on("data", (chunk) => { diagnostics += chunk; });
  try {
    return await new Promise((accept, reject) => {
      const timer = setTimeout(() => reject(new Error(`MCP smoke timed out: ${diagnostics}`)), 10_000);
      const fail = (error) => { clearTimeout(timer); reject(error); };
      child.on("error", fail);
      child.on("exit", (code) => fail(new Error(`MCP exited (${code}): ${diagnostics}`)));
      child.stdin.on("error", fail);
      const send = (value) => child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", ...value })}\n`);
      lines.on("line", (line) => {
        try {
          const message = JSON.parse(line);
          assert(!message.error, JSON.stringify(message.error));
          if (message.id === 1) {
            assert.equal(message.result.serverInfo.name, name);
            assert.equal(message.result.serverInfo.version, expectedVersion);
            assert.equal(message.result.protocolVersion, "2025-11-25");
            send({ method: "notifications/initialized" });
            send({ id: 2, method: "tools/list", params: {} });
          } else if (message.id === 2) {
            const tools = message.result.tools;
            assert(tools.length > 0, "No MCP tools discovered");
            assert.equal(new Set(tools.map((tool) => tool.name)).size, tools.length);
            for (const tool of tools) assert.equal(tool.inputSchema.type, "object");
            clearTimeout(timer);
            accept(tools);
          }
        } catch (error) { fail(error); }
      });
      // rmcp's newest protocol omits initialize; this checks desktop-compatible negotiation.
      send({ id: 1, method: "initialize", params: {
        protocolVersion: "2025-11-25", capabilities: {}, clientInfo: { name: "package-smoke", version: "1" },
      } });
    });
  } finally {
    lines.close();
    child.stdin.destroy();
    child.kill("SIGKILL");
  }
}

console.log(`Building ${label}...`);
run("cargo", ["build", "--package", name, "--release", "--locked", "--target", target], { stdio: "inherit", timeout: 600_000 });
const binary = join(metadata.target_directory, target, "release", name);
assert.equal(run("/usr/bin/lipo", ["-archs", binary]), process.arch === "x64" ? "x86_64" : "arm64");
const expectedTools = await smoke(binary, args, repo, version);
const notices = thirdPartyNotices(repo, target, name);
console.log(`Third-party notices: ${notices.count} crates`);
const output = mkdtempSync(join(tmpdir(), `${name}-packages-`));
const staging = mkdtempSync(join(tmpdir(), `${name}-staging-`));

// Each bundle carries the binary with the license, notice and dependency texts it requires.
function copyBinary(root) {
  const destination = join(root, entryPoint);
  mkdirSync(dirname(destination), { recursive: true });
  copyFileSync(binary, destination);
  chmodSync(destination, 0o755);
  for (const file of ["LICENSE", "NOTICE"]) copyFileSync(join(repo, file), join(root, file));
  writeFileSync(join(root, "THIRD_PARTY_NOTICES.txt"), notices.text);
}

try {
  const claude = join(staging, "claude");
  copyBinary(claude);
  const manifest = {
    manifest_version: "0.3", name, version, description, author,
    display_name: `Wanderlog (macOS ${process.arch})`,
    server: {
      type: "binary", entry_point: entryPoint,
      mcp_config: {
        command: `\${__dirname}/${entryPoint}`, args,
        env: { WANDERLOG_COOKIE: "${user_config.cookie}" },
      },
    },
    compatibility: { platforms: ["darwin"] },
    privacy_policies: ["https://github.com/cebrusfs/wanderlog-mcp-rs/blob/main/PRIVACY.md"],
    tools: expectedTools.map(({ name, description }) => ({ name, description })),
    user_config: {
      cookie: {
        type: "string", title: "Wanderlog session cookie (optional)",
        description: "Leave blank to use your existing macOS Keychain session. A supplied cookie overrides it for this extension only.",
        required: false, default: "", sensitive: true,
      },
    },
  };
  json(join(claude, "manifest.json"), manifest);
  const mcpb = join(output, `${label}.mcpb`);
  assert(await packExtension({ extensionPath: claude, outputPath: mcpb, silent: true }), "MCPB packing failed");

  const marketplace = join(output, `${label}-openai`);
  const plugin = join(marketplace, "plugins", name);
  copyBinary(plugin);
  const schemaRoot = "https://agent-plugins.org/schemas/1.0.0";
  const pluginManifest = {
    $schema: `${schemaRoot}/plugin.schema.json`, name, version, description, author,
    extensions: { "com.openai": { interface: {
      displayName: `Wanderlog (macOS ${process.arch})`,
      shortDescription: "Read and edit your Wanderlog trips", category: "Productivity",
    } } },
  };
  const mcp = {
    $schema: `${schemaRoot}/mcp.schema.json`,
    mcpServers: { wanderlog: { type: "stdio", command: `./${entryPoint}`, args } },
  };
  await Promise.all([validatePortable(pluginManifest), validatePortable(mcp)]);
  json(join(plugin, "plugin.json"), pluginManifest);
  json(join(plugin, "mcp.json"), mcp);
  json(join(marketplace, ".agents/plugins/marketplace.json"), {
    name: "wanderlog-local", interface: { displayName: "Wanderlog local" },
    plugins: [{ name, source: { source: "local", path: `./plugins/${name}` },
      policy: { installation: "AVAILABLE", authentication: "ON_INSTALL" }, category: "Productivity" }],
  });
  for (const path of ["README.md", "LICENSE", "NOTICE", "PRIVACY.md", "docs/tools.md", "docs/development.md", "docs/desktop.md", "docs/protocol.md"]) {
    mkdirSync(dirname(join(marketplace, path)), { recursive: true });
    copyFileSync(join(repo, path), join(marketplace, path));
  }
  const zip = `${marketplace}.zip`;
  run("/usr/bin/ditto", ["-c", "-k", "--keepParent", "--norsrc", marketplace, zip]);

  // Re-extract into paths containing spaces, as real client caches often do.
  const claudeExtracted = join(staging, "Claude installed");
  assert(await unpackExtension({ mcpbPath: mcpb, outputDir: claudeExtracted, silent: true }));
  const installedManifest = readJson(join(claudeExtracted, "manifest.json"));
  const configOptions = { manifest: installedManifest, extensionPath: claudeExtracted, systemDirs: {}, pathSeparator: "/" };
  const config = await getMcpConfigForManifest({ ...configOptions, userConfig: {} });
  assert(config, "Optional cookie must not block startup");
  assert.equal(config.env.WANDERLOG_COOKIE, "");
  const supplied = await getMcpConfigForManifest({ ...configOptions, userConfig: { cookie: "test-cookie" } });
  assert.equal(supplied.env.WANDERLOG_COOKIE, "test-cookie");
  assert.deepEqual(await smoke(config.command, config.args, claudeExtracted, installedManifest.version), expectedTools);

  const openaiExtracted = join(staging, "OpenAI installed");
  run("/usr/bin/ditto", ["-x", "-k", zip, openaiExtracted]);
  const installedRoot = join(openaiExtracted, `${label}-openai`);
  const catalog = readJson(join(installedRoot, ".agents/plugins/marketplace.json"));
  const installedPlugin = resolve(installedRoot, catalog.plugins[0].source.path);
  const installedMcp = readJson(join(installedPlugin, "mcp.json")).mcpServers.wanderlog;
  assert(installedMcp.command.startsWith("./"), "Bundled commands must be plugin-relative");
  assert.deepEqual(await smoke(resolve(installedPlugin, installedMcp.command), installedMcp.args, installedPlugin,
    readJson(join(installedPlugin, "plugin.json")).version), expectedTools);
  for (const installed of [join(claudeExtracted, entryPoint), join(installedPlugin, entryPoint)]) {
    assert(readFileSync(binary).equals(readFileSync(installed)), "Archive changed the binary");
  }
  console.log(`Verified both extracted packages: ${expectedTools.length} MCP tools, optional cookie, executable permissions, relocation, matching binaries.`);
  console.log(`Artifacts: ${output}`);
} finally {
  rmSync(staging, { recursive: true, force: true });
}
