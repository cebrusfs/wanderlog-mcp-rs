#!/usr/bin/env python3
"""Check release names, versions, notices and package boundaries, without network."""
from pathlib import Path
import argparse
import re
import tomllib


def require(condition, message):
    if not condition:
        raise ValueError(message)


def check(root, tag=None):
    app = tomllib.loads((root / 'Cargo.toml').read_text())
    client = tomllib.loads((root / 'crates/wanderlog-client/Cargo.toml').read_text())
    version = app['package']['version']
    require(app['package']['name'] == 'wanderlog-mcp', 'Unexpected MCP name')
    require(client['package']['name'] == 'wanderlog-client', 'Unexpected client name')
    require(client['package']['version'] == version, 'Release versions differ')
    require(re.fullmatch(r'\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?', version), 'Invalid version')
    require(app['workspace']['package']['license'] == 'Apache-2.0', 'Unexpected license')
    for document in (app, client):
        require(document['package']['license'] == {'workspace': True}, 'License not inherited')
        require(document['package']['publish'] == ['crates-io'], 'Unexpected registry')
    for group in ('dependencies', 'dev-dependencies'):
        dep = app[group]['wanderlog-client']
        require(dep['version'] == f'={version}', 'Client version requirement differs')
        require(dep['path'] == 'crates/wanderlog-client', 'Client path differs')
    require(not {'rmcp', 'clap', 'keyring', 'rpassword'} & set(client['dependencies']),
            'Client depends on application tooling')
    for name in ('LICENSE', 'NOTICE'):
        require((root / name).read_bytes() == (root / 'crates/wanderlog-client' / name).read_bytes(),
                f'Mismatched {name}')
    if tag is not None:
        require(tag == f'v{version}', 'Tag does not match release version')
    return version


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tag')
    args = parser.parse_args()
    print('Release metadata verified:', check(Path(__file__).resolve().parents[1], args.tag))
