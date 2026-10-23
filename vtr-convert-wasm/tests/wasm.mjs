import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { webcrypto } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { gzipSync } from 'node:zlib';

const [bindings, cli] = process.argv.slice(2);
assert.ok(bindings && cli, 'usage: node tests/wasm.mjs BINDINGS_DIR VTR_CLI');
globalThis.crypto ??= webcrypto;
let progress = [];
globalThis.volnaConversionProgress = json => progress.push(JSON.parse(json));
const require = createRequire(import.meta.url);
const converter = require(resolve(bindings, 'vtr_convert_wasm.js'));
assert.equal(converter.max_input_bytes(), 256 * 1024 * 1024);
const fixtures = resolve(dirname(fileURLToPath(import.meta.url)), '../../vtr-convert/tests/fixtures');
const temporary = mkdtempSync(join(tmpdir(), 'vtr-wasm-'));
try {
    for (const name of ['pipeline.fst', 'pipeline.kanata', 'waves.vcd']) {
        const input = readFileSync(join(fixtures, name));
        const nativePath = join(temporary, `${name}.native.vtr`);
        execFileSync(resolve(cli), ['convert', join(fixtures, name), nativePath, '--progress=none']);
        const native = readFileSync(nativePath);
        for (const compressed of [false, true]) {
            progress = [];
            const bytes = Buffer.from(converter.convert(compressed ? `${name}.gz` : name, compressed ? gzipSync(input) : input));
            assert.deepEqual(bytes.subarray(0, 16), native.subarray(0, 16));
            assert.notDeepEqual(bytes.subarray(16, 32), native.subarray(16, 32));
            assert.deepEqual(bytes.subarray(32), native.subarray(32));
            assert.equal(bytes.readUInt16LE(8), 3);
            assert.equal(bytes.readUInt16LE(10), 2);
            const output = join(temporary, `${name}.wasm.vtr`);
            writeFileSync(output, bytes);
            execFileSync(resolve(cli), ['info', output]);
            const final = progress.at(-1);
            assert.equal(final.phase, 'complete');
            assert.equal(final.input_bytes, compressed ? gzipSync(input).length : input.length);
            assert.equal(final.output_bytes, bytes.length);
            assert.ok(progress.some(snapshot => snapshot.phase === 'reading'));
            assert.ok(progress.some(snapshot => snapshot.phase === 'finalizing'));
        }
        console.log(`WASM/native observations and progress agree: ${name}`);
    }
    for (const [name, input] of [['bad.fst', Buffer.from('bad')], ['bad.kanata', Buffer.from('Kanata\t0004\nC=\t9\nC=\t8\n')], ['bad.vcd', Buffer.from('bad')]]) {
        progress = [];
        assert.throws(() => converter.convert(name, input));
        assert.equal(progress.at(-1).phase, 'failed');
        assert.ok(progress.every(snapshot => snapshot.phase !== 'complete'));
    }
} finally {
    rmSync(temporary, { recursive: true, force: true });
}
