import { createHash } from 'node:crypto';
import { mkdir, readFile, writeFile, rm } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import path from 'node:path';

const pins = {
  darwin: {
    url: 'https://github.com/sparkle-project/Sparkle/releases/download/2.10.0/Sparkle-2.10.0.tar.xz',
    sha256: 'c2bf58aa8387266ac179357b1415d6f2635f044da8be41042af32425dae6da0c',
    archive: 'Sparkle.tar.xz',
  },
  win32: {
    url: 'https://github.com/vslavik/winsparkle/releases/download/v0.9.4/WinSparkle-0.9.4.zip',
    sha256: '6037df37fc263bd1650a1c4949681a9d40ffe991d01f35892a406cb5d103c976',
    archive: 'WinSparkle.zip',
  },
};
const pin = pins[process.platform];
if (!pin) throw new Error('Native updater dependencies require macOS or Windows.');
const directory = path.resolve('.cache/updaters');
await mkdir(directory, { recursive: true });
const response = await fetch(pin.url);
if (!response.ok) throw new Error(`Updater download failed: ${response.status}`);
const bytes = Buffer.from(await response.arrayBuffer());
if (createHash('sha256').update(bytes).digest('hex') !== pin.sha256) {
  throw new Error('Updater archive checksum mismatch.');
}
const archive = path.join(directory, pin.archive);
await writeFile(archive, bytes);
if (process.platform === 'darwin') {
  execFileSync('tar', ['-xf', archive, '-C', directory], { stdio: 'inherit' });
} else {
  execFileSync('tar', ['-xf', archive, '-C', directory], { stdio: 'inherit' });
  const root = path.join(directory, 'WinSparkle-0.9.4');
  for (const architecture of ['x64', 'ARM64']) {
    const dll = await readFile(path.join(root, architecture, 'Release/WinSparkle.dll'));
    const pe = dll.readUInt32LE(0x3c);
    if (dll.subarray(pe, pe + 4).toString('hex') !== '50450000'
      || dll.readUInt16LE(pe + 4) !== (architecture === 'x64' ? 0x8664 : 0xaa64)) {
      throw new Error(`Wrong WinSparkle DLL architecture: ${architecture}`);
    }
  }
}
await rm(archive);
console.log(`Verified native updater dependency in ${directory}`);
