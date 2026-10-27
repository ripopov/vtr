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

function observations(file) {
    const inspect = (command, ...args) => execFileSync(resolve(cli), [command, file, ...args], { encoding: 'utf8' });
    const hierarchy = inspect('hierarchy', '--vars');
    const ancestors = [];
    const signals = {};
    for (const line of hierarchy.split('\n')) {
        const signal = /^( *)(.*?) : /.exec(line);
        if (signal) {
            const path = [...ancestors.slice(0, signal[1].length / 2), signal[2]].join('.');
            signals[path] = inspect('changes', path, '--max', '1000000');
        } else {
            const node = /^( *)(.*?) \[/.exec(line);
            if (node) ancestors[node[1].length / 2] = node[2];
        }
    }
    const transactions = inspect('transactions', '--max', '1000000');
    const ids = [...transactions.matchAll(/^record (\d+) .*kind=Transaction(?: |$)/gm)].map(match => match[1]);
    return {
        metadata: inspect('info').split('\n').filter(line => !/^(file:|recording:|section )/.test(line)),
        hierarchy,
        signals,
        transactions,
        records: ids.length ? inspect('transactions', '--id', ids.join(',')) : '',
        clocks: inspect('clocks'),
    };
}

try {
    for (const name of ['pipeline.fst', 'pipeline.kanata', 'waves.vcd']) {
        const input = readFileSync(join(fixtures, name));
        const nativePath = join(temporary, `${name}.native.vtr`);
        execFileSync(resolve(cli), ['convert', join(fixtures, name), nativePath, '--progress=none']);
        const native = readFileSync(nativePath);
        const expected = observations(nativePath);
        for (const compressed of [false, true]) {
            progress = [];
            const bytes = Buffer.from(converter.convert(compressed ? `${name}.gz` : name, compressed ? gzipSync(input) : input));
            assert.deepEqual(bytes.subarray(0, 16), native.subarray(0, 16));
            assert.notDeepEqual(bytes.subarray(16, 32), native.subarray(16, 32));
            assert.equal(bytes.readUInt16LE(8), 3);
            assert.equal(bytes.readUInt16LE(10), 4);
            const output = join(temporary, `${name}.wasm.vtr`);
            writeFileSync(output, bytes);
            // Native and WASM LZ4 encoders can produce different compressed bytes.
            assert.deepEqual(observations(output), expected, `${name}: decoded WASM/native content`);
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
