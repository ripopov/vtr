#!/usr/bin/env python3
"""Enforce trace/server boundaries and run their standalone headless checks."""
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def run(*args, capture=False):
    return subprocess.run(args, cwd=ROOT, check=True, timeout=900,
                          text=True, stdout=subprocess.PIPE if capture else None).stdout


def check_graph():
    # Metadata resolves all target-specific and development dependencies. A GUI
    # dependency cannot hide behind a platform cfg or a test-only feature.
    metadata = json.loads(run('cargo', 'metadata', '--locked', '--format-version', '1', '--all-features', capture=True))
    packages = {p['id']: p for p in metadata['packages']}
    nodes = {n['id']: n for n in metadata['resolve']['nodes']}
    by_name = {p['name']: p for p in metadata['packages'] if p['id'] in metadata['workspace_members']}
    toolkits = ('gpui', 'egui', 'eframe', 'winit', 'iced', 'slint', 'gtk', 'qt')
    for name in ('volna-trace', 'volna-server'):
        root = by_name[name]
        visited = set()
        todo = [root['id']]
        while todo:
            identity = todo.pop()
            if identity in visited:
                continue
            visited.add(identity)
            dependency = packages[identity]['name']
            assert dependency not in ('volna-core', 'volna', 'volna-egui'), (name, dependency)
            assert not dependency.startswith(toolkits), (name, dependency)
            if name == 'volna-trace':
                assert dependency != 'volna-server', (name, dependency)
            todo.extend(d['pkg'] for d in nodes[identity]['deps'])
        print(f'{name}: {len(visited)} packages; no viewer or toolkit dependency', flush=True)
    assert all('bin' not in t['kind'] for t in by_name['volna-trace']['targets'])
    assert all('lib' not in t['kind'] for t in by_name['volna-server']['targets'])
    for owner, dependency in [('volna-core', 'volna-trace'), ('volna-server', 'volna-trace'),
                              ('volna', 'volna-core'), ('volna-egui', 'volna-core')]:
        assert by_name[dependency]['id'] in {d['pkg'] for d in nodes[by_name[owner]['id']]['deps']}, (owner, dependency)


if __name__ == '__main__':
    assert sys.argv[1:] in ([], ['--graph-only']), 'usage: check-volna-loading.py [--graph-only]'
    check_graph()
    if not sys.argv[1:]:
        run('cargo', 'build', '--locked', '-p', 'volna-trace', '-p', 'volna-server', '--all-targets')
        run('cargo', 'test', '--locked', '-p', 'volna-trace', '-p', 'volna-server', '--all-targets')
        run('cargo', 'test', '--locked', '-p', 'volna-trace', '--doc')
        run('cargo', 'clippy', '--locked', '-p', 'volna-trace', '-p', 'volna-server',
            '--all-targets', '--all-features', '--', '-D', 'warnings')
