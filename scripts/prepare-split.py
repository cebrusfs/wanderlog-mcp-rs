#!/usr/bin/env python3
"""One-time guarded split; no network, credentials, releases or workflow edits."""
from pathlib import Path
import hashlib
import re
import textwrap
import tomllib

ROOT = Path(__file__).resolve().parents[1]
EXPECTED = {
 'Cargo.toml':'a676404b42d3eadb8baa8f0acfa8f0567590203b',
 'Cargo.lock':'b6afe61e6d56fbd9fb2fdf31dcc791dc7c1d7092',
 'src/lib.rs':'c2d66baaa54b088d49f065ff576348c0beabd7c4',
 'src/auth.rs':'b810aec72ff51ffd52e3a2cb033cc2b2a0fbfedb',
 'src/dates.rs':'e1ca22cea87c5a2f2d94acc72381a80f0376ffb2',
 'src/edit.rs':'dcbf3102738aade86d74b72879ae2177f343190e',
 'src/json0.rs':'cff213ac4f8f40c45229b63b775847e339db3e3d',
 'src/render.rs':'d6c6bccc50fafea6492227011c5636b78680e1d4',
 'src/rest.rs':'3fa20be795e398f5b44e5cf59781fcec8e0ebf90',
 'src/sharedb.rs':'12296f3cdd874ef3f8584c0bb8999d6dcd916fd8',
 'src/trip.rs':'d1dc8e0459d2b90968a2267c54706b25b93eeaac',
 'src/server.rs':'dc89c38d39b2df01d4cfa8e22aa4936f738d0ca1',
 'mise.toml':'a98a84433fa59872cfb19a6d64b900bd69229284',
 'deny.toml':'a916971b9b9783e3a3a3ca8d5ee16bb6fe4b0fdd',
 'scripts/package.mjs':'11779ed9d9778a8143d6e0246da383ad1759163b',
 'README.md':'1a9423d8f3a99f10a347c541c190dbb5b13e8c06',
 'docs/development.md':'253b4c05fabdcf9e26fcc404037a79cbdebbfd4d',
}
original = {}
for path, expected in EXPECTED.items():
    data = (ROOT / path).read_bytes()
    actual = hashlib.sha1(b'blob ' + str(len(data)).encode() + b'\0' + data).hexdigest()
    if actual != expected:
        raise ValueError(f'Base changed; refusing overwrite: {path}')
    original[path] = data.decode()


def once(source, old, new):
    if source.count(old) != 1:
        raise ValueError(f'Unexpected migration anchor: {old[:80]}')
    return source.replace(old, new)


changes = {}
modules = ('dates','edit','json0','render','rest','sharedb','trip')
for module in modules:
    changes[f'crates/wanderlog-client/src/{module}.rs'] = original[f'src/{module}.rs']
    changes[f'src/{module}.rs'] = None
changes['crates/wanderlog-client/src/edit.rs'] = once(original['src/edit.rs'],
    'use rmcp::schemars::{self, JsonSchema};', 'use schemars::JsonSchema;')
auth = original['src/auth.rs']
a, b, t = auth.index('/// Accept `connect.sid='), auth.index('fn entry()'), auth.index('#[cfg(test)]')
changes['crates/wanderlog-client/src/auth.rs'] = (
    '//! Cookie validation only; no persistence.\n\nuse anyhow::{Result, bail};\n\n' + auth[a:b] + auth[t:])
changes['src/auth.rs'] = auth[:a] + 'pub use wanderlog_client::auth::normalize;\n\n' + auth[b:t]
rest = original['src/rest.rs']
marker = '#[cfg(test)]\npub(crate) mod tests {'
if rest.count(marker) != 1:
    raise ValueError('Unexpected REST tests')
production, tests = rest.split(marker)
production = production.replace('#[cfg(test)]', '#[cfg(any(test, feature = "test-support"))]')
production = production.replace('#[cfg(not(test))]', '#[cfg(not(any(test, feature = "test-support")))]')
production = once(production, 'pub(crate) fn for_test', 'pub fn for_test')
a, b = tests.index('    pub(crate) fn response('), tests.index('    #[tokio::test]')
helpers = textwrap.dedent(tests[a:b]).replace('pub(crate) ', 'pub ')
changes['crates/wanderlog-client/src/test_support.rs'] = (
    '//! Synthetic loopback helpers; not a stable API.\n\nuse std::time::Duration;\n'
    'use tokio::io::{AsyncReadExt, AsyncWriteExt};\n\n' + helpers)
tests = tests[:a] + tests[b:]
traits = '    use tokio::io::{AsyncReadExt, AsyncWriteExt};'
imports = '    use crate::test_support::{login_server, response};'
if re.search(r'\.(read|write_all)\(', tests):
    imports = traits + '\n' + imports
tests = once(tests, traits, imports)
changes['crates/wanderlog-client/src/rest.rs'] = production + marker + tests
changes['src/server.rs'] = once(original['src/server.rs'],
    'use crate::rest::tests::{login_server, response};',
    'use wanderlog_client::test_support::{login_server, response};')
changes['crates/wanderlog-client/src/trip.rs'] = once(original['src/trip.rs'],
    '#[cfg(test)]\npub(crate) mod fixture {',
    '#[cfg(any(test, feature = "test-support"))]\n#[doc(hidden)]\npub mod fixture {')
changes['src/lib.rs'] = '''//! Local stdio MCP, CLI and operating-system credential persistence.
//! Prefer `wanderlog-client` for programmatic access.
//! Re-exports preserve legacy public module paths.

pub mod auth;
pub mod server;

pub use wanderlog_client::{USER_AGENT, dates, edit, json0, render, rest, sharedb, trip};
'''
changes['Cargo.toml'] = '''[workspace]
members = ["crates/wanderlog-client"]
default-members = [".", "crates/wanderlog-client"]
resolver = "3"

[workspace.package]
edition = "2024"
rust-version = "1.89"
license = "Apache-2.0"
repository = "https://github.com/cebrusfs/wanderlog-mcp-rs"

[workspace.dependencies]
anyhow = "1.0.104"
serde = { version = "1.0.229", features = ["derive"] }
serde_json = "1.0.151"
tokio = { version = "1.53.2", features = ["rt-multi-thread", "macros", "io-std", "time", "sync"] }

[package]
name = "wanderlog-mcp"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
description = "Unofficial local MCP server and CLI for Wanderlog trips"
readme = "README.md"
keywords = ["wanderlog", "mcp", "travel", "itinerary", "cli"]
categories = ["command-line-utilities"]
publish = ["crates-io"]
include = ["src/**", "tests/**", "Cargo.toml", "Cargo.lock", "README.md", "LICENSE", "NOTICE", "PRIVACY.md", "SECURITY.md", "CONTRIBUTING.md", "CHANGELOG.md", "docs/**"]

[package.metadata.docs.rs]
default-target = "aarch64-apple-darwin"
targets = ["aarch64-apple-darwin"]

[dependencies]
anyhow.workspace = true
clap = { version = "4.6.7", features = ["derive"] }
keyring = "4.2.0"
rmcp = { version = "3.5.1", features = ["transport-io"] }
rpassword = "7.5.4"
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
wanderlog-client = { path = "crates/wanderlog-client", version = "=0.1.0" }

[dev-dependencies]
wanderlog-client = { path = "crates/wanderlog-client", version = "=0.1.0", features = ["test-support"] }
'''
changes['crates/wanderlog-client/NOTICE'] = (ROOT / 'NOTICE').read_text()
lock = original['Cargo.lock']
packages = tomllib.loads(lock)['package']
old = next(p for p in packages if p['name'] == 'wanderlog-mcp')
if old['version'] != '0.1.0' or not any(p['name'] == 'schemars' and p['version'] == '1.2.2' for p in packages):
    raise ValueError('Unexpected lock versions')
deps = {d.split()[0]: d for d in old['dependencies']}
client_deps = [deps[n] for n in ('anyhow','futures-util','httpdate','rand','reqwest','serde','serde_json','tokio','tokio-tungstenite')] + ['schemars']
app_deps = [deps[n] for n in ('anyhow','clap','keyring','rmcp','rpassword','serde','serde_json','tokio')] + ['wanderlog-client']

def stanza(name, dependencies):
    return f'[[package]]\nname = "{name}"\nversion = "0.1.0"\ndependencies = [\n' + ''.join(f' "{d}",\n' for d in sorted(dependencies)) + ']\n\n'

lock, count = re.subn(r'\[\[package\]\]\nname = "wanderlog-mcp"\n.*?(?=\[\[package\]\]|\Z)',
    lambda _: stanza('wanderlog-client',client_deps)+stanza('wanderlog-mcp',app_deps), lock, flags=re.S)
if count != 1:
    raise ValueError('Unexpected lock layout')
changes['Cargo.lock'] = lock
mise = original['mise.toml']
for old, new in [
 ('cargo fmt --check','cargo fmt --all --check'),
 ('cargo clippy --locked --all-targets','cargo clippy --workspace --locked --all-targets'),
 ('cargo test --locked --lib --bins','cargo test --workspace --locked --lib --bins'),
 ('cargo test --locked --doc','cargo test --workspace --locked --doc'),
 ('cargo test --test e2e','cargo test -p wanderlog-mcp --test e2e'),
 ('cargo llvm-cov --locked','cargo llvm-cov --workspace --locked')]:
    mise = once(mise,old,new)
mise = once(mise, 'run = ["mise run lint-ci", "mise run check"]',
    'run = ["mise run lint-ci", "python3 scripts/release-check.py", "mise run check"]')
changes['mise.toml'] = mise
changes['deny.toml'] = once(original['deny.toml'],
 '[licenses.private]\n# This crate is unpublished (publish = false) and has no license field.\nignore = true\n\n','')
package = once(original['scripts/package.mjs'],
 '["build", "--release", "--locked", "--target", target]',
 '["build", "--package", "wanderlog-mcp", "--release", "--locked", "--target", target]')
package = once(package, 'function copyBinary(root) {',
 'function copyBinary(root) {\n  mkdirSync(root, { recursive: true });\n'
 '  for (const file of ["LICENSE", "NOTICE", "PRIVACY.md"]) {\n'
 '    copyFileSync(join(repo, file), join(root, file));\n  }')
package = once(package, 'privacy_policies: ["https://wanderlog.com/privacy"],',
 'privacy_policies: ["https://github.com/cebrusfs/wanderlog-mcp-rs/blob/main/PRIVACY.md"],')
changes['scripts/package.mjs'] = package
for path in ('README.md','docs/development.md'):
    text = original[path]
    for m in modules:
        text = text.replace(f'src/{m}.rs',f'crates/wanderlog-client/src/{m}.rs')
    text += '''
## Rust workspace and releases

`wanderlog-client` (`wanderlog_client`) provides the reusable API without MCP or
Keychain. `wanderlog-mcp` retains the local stdio server, CLI and credentials.
The repository and executable names remain unchanged; legacy module paths are
re-exported. New Rust consumers should use the client crate directly.

Both crates use Apache-2.0; see LICENSE and NOTICE. This grants code rights,
not upstream service, data, trademark or official affiliation rights. See
[release instructions](https://github.com/cebrusfs/wanderlog-mcp-rs/blob/main/docs/publishing.md),
CONTRIBUTING.md, SECURITY.md and PRIVACY.md. A Cargo name is not a registry
reservation. Remote MCP is out of scope. No release is triggered by a push.
'''
    changes[path] = text
for path, content in changes.items():
    target = ROOT / path
    if any(p.is_symlink() for p in (target,*target.parents)):
        raise ValueError(f'Symlink not allowed: {path}')
    if path not in original and target.exists():
        raise ValueError(f'New file already exists: {path}')
    if content is not None and (path.endswith('.toml') or path == 'Cargo.lock'):
        tomllib.loads(content)
for path, content in changes.items():
    if content is not None:
        target = ROOT / path
        target.parent.mkdir(parents=True,exist_ok=True)
        target.write_bytes(content.encode())
for path, content in changes.items():
    if content is None:
        (ROOT / path).unlink()
print(f'Applied {len(changes)} guarded source changes; no live requests.')
