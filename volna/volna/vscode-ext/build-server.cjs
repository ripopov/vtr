#!/usr/bin/env node
// Build the native child for this workspace host and install it at the relay's
// platform-specific path. Remote-host packages must be built on that host.
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');
const { serverBinary } = require('./relay');
const root = path.resolve(__dirname, '../../..');
const options = { cwd: root, encoding: 'utf8' };
const target = execFileSync('rustc', ['-vV'], options).split('\n')
  .find(line => line.startsWith('host: '))?.slice(6).trim();
const arch = target?.startsWith('aarch64-') ? 'arm64' : target?.startsWith('x86_64-') ? 'x64' : undefined;
const platform = target?.includes('-apple-darwin') ? 'darwin'
  : target?.includes('-unknown-linux-') ? 'linux'
  : target?.includes('-pc-windows-') ? 'win32' : undefined;
if (arch !== process.arch || platform !== process.platform) {
  throw new Error(`Rust host ${target} does not match Node host ${process.platform}-${process.arch}; run both toolchains for the target workspace host`);
}
const metadata = JSON.parse(execFileSync('cargo', ['metadata', '--no-deps', '--format-version', '1'], options));
// An explicit target avoids silently packaging a CARGO_BUILD_TARGET override.
execFileSync('cargo', ['build', '--locked', '-p', 'vtr-server', '--release', '--target', target],
  { cwd: root, stdio: 'inherit' });
const destination = serverBinary(__dirname, platform, arch);
const source = path.join(metadata.target_directory, target, 'release', path.basename(destination));
fs.mkdirSync(path.dirname(destination), { recursive: true });
fs.copyFileSync(source, destination);
if (platform !== 'win32') fs.chmodSync(destination, 0o755);
console.log(destination);
