import { createHash, createPrivateKey, createPublicKey, sign, verify } from 'node:crypto';
import { readFile, writeFile, mkdir, mkdtemp, rm, appendFile } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import path from 'node:path';

const repository = 'listenbox/client';
const platforms = ['macos-arm64', 'windows-x64', 'windows-arm64'];
const [command, ...args] = process.argv.slice(2);

function versionParts(version) {
  if (!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version)) {
    throw new Error(`Invalid stable version: ${version}`);
  }
  const parts = version.split('.').map(Number);
  if (parts.some(value => value > 65535)) throw new Error('Version component exceeds Windows limit.');
  return parts;
}

function compareVersions(left, right) {
  const a = versionParts(left);
  const b = versionParts(right);
  for (let i = 0; i < 3; i++) if (a[i] !== b[i]) return a[i] - b[i];
  return 0;
}

async function identity(tag) {
  const manifest = await readFile('Cargo.toml', 'utf8');
  const workspace = manifest.split('[workspace.package]')[1]?.split(/\n\[/)[0];
  const version = workspace?.match(/^version = "([^"]+)"$/m)?.[1];
  versionParts(version);
  const commit = execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
  if (tag && tag !== `v${version}`) throw new Error('Tag and workspace version disagree.');
  if (tag && execFileSync('git', ['rev-parse', `${tag}^{commit}`], { encoding: 'utf8' }).trim() !== commit) {
    throw new Error('Tag does not point at the built commit.');
  }
  return { version, windowsVersion: `${version}.0`, commit, tag: `v${version}` };
}

function assetName(release, platform) {
  return `Listenbox-${release.version}-${platform}${platform.startsWith('windows') ? '-setup.exe' : '.dmg'}`;
}

function rawKey(value, length) {
  if (typeof value !== 'string' || Buffer.from(value, 'base64').length !== length
      || Buffer.from(value, 'base64').toString('base64') !== value) {
    throw new Error(`Expected canonical base64 for a ${length}-byte signing key.`);
  }
  return Buffer.from(value, 'base64');
}

function publicKey(platform) {
  const name = platform.startsWith('macos') ? 'MACOS_UPDATE_PUBLIC_KEY' : 'WINDOWS_UPDATE_PUBLIC_KEY';
  return createPublicKey({ key: Buffer.concat([
    Buffer.from('302a300506032b6570032100', 'hex'), rawKey(process.env[name], 32),
  ]), format: 'der', type: 'spki' });
}

function xml(value) {
  return String(value).replaceAll('&', '&amp;').replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;').replaceAll('"', '&quot;').replaceAll("'", '&apos;');
}

function appcast(manifest) {
  const windows = manifest.platform.startsWith('windows');
  const buildVersion = windows ? `${manifest.version}.0` : manifest.version;
  const base = `https://github.com/${repository}/releases`;
  return `<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
<channel><title>Listenbox ${xml(manifest.platform)}</title><link>https://listenbox.app</link><description>Stable desktop updates</description><language>en</language>
<item><title>Listenbox ${xml(manifest.version)}</title>
<sparkle:version>${xml(buildVersion)}</sparkle:version>
<sparkle:shortVersionString>${xml(manifest.version)}</sparkle:shortVersionString>
<sparkle:minimumSystemVersion>${windows ? '10.0.22000' : '13.0'}</sparkle:minimumSystemVersion>
<enclosure url="${base}/download/${xml(manifest.tag)}/${xml(manifest.asset)}" length="${manifest.length}" type="application/octet-stream" sparkle:version="${xml(buildVersion)}" sparkle:shortVersionString="${xml(manifest.version)}" sparkle:edSignature="${xml(manifest.signature)}" sparkle:os="${windows ? xml(manifest.platform) : 'macos'}"${windows ? ' sparkle:installerArguments="/SILENT /SP- /NOICONS /NORESTART /UPGRADE"' : ''}/>
</item></channel></rss>
`;
}

async function prepare(platform, artifact, directory) {
  if (!platforms.includes(platform)) throw new Error('Unsupported update platform.');
  const release = await identity(process.env.GITHUB_REF_TYPE === 'tag' ? process.env.GITHUB_REF_NAME : undefined);
  if (path.basename(artifact) !== assetName(release, platform)) throw new Error('Unexpected payload name.');
  const seedName = platform.startsWith('macos') ? 'MACOS_UPDATE_SEED_BASE64' : 'WINDOWS_UPDATE_SEED_BASE64';
  const privateKey = createPrivateKey({ key: Buffer.concat([
    Buffer.from('302e020100300506032b657004220420', 'hex'), rawKey(process.env[seedName], 32),
  ]), format: 'der', type: 'pkcs8' });
  const key = publicKey(platform);
  if (!createPublicKey(privateKey).equals(key)) throw new Error('Signing seed and embedded public key disagree.');
  const bytes = await readFile(artifact);
  if (bytes.length === 0) throw new Error('Empty update payload.');
  const signature = sign(null, bytes, privateKey);
  if (!verify(null, bytes, key, signature)) throw new Error('Payload signature verification failed.');
  const manifest = {
    ...release, platform, asset: path.basename(artifact), length: bytes.length,
    sha256: createHash('sha256').update(bytes).digest('hex'), signature: signature.toString('base64'),
  };
  await mkdir(directory, { recursive: true });
  await writeFile(path.join(directory, manifest.asset), bytes);
  await writeFile(path.join(directory, `manifest-${platform}.json`), `${JSON.stringify(manifest, null, 2)}\n`);
  await writeFile(path.join(directory, `appcast-${platform}.xml`), appcast(manifest));
}

async function verifyArtifacts(directory, release) {
  const files = [];
  for (const platform of platforms) {
    const manifestName = `manifest-${platform}.json`;
    const feedName = `appcast-${platform}.xml`;
    const manifest = JSON.parse(await readFile(path.join(directory, manifestName), 'utf8'));
    if (manifest.version !== release.version || manifest.commit !== release.commit
        || manifest.tag !== release.tag || manifest.platform !== platform
        || manifest.asset !== assetName(release, platform)) throw new Error('Release identity mismatch.');
    const bytes = await readFile(path.join(directory, manifest.asset));
    if (!bytes.length || bytes.length !== manifest.length
        || createHash('sha256').update(bytes).digest('hex') !== manifest.sha256
        || !verify(null, bytes, publicKey(platform), rawKey(manifest.signature, 64))) {
      throw new Error(`Invalid payload bytes/signature: ${platform}`);
    }
    if (await readFile(path.join(directory, feedName), 'utf8') !== appcast(manifest)) {
      throw new Error(`Appcast differs from verified payload: ${platform}`);
    }
    files.push(manifest.asset, manifestName, feedName);
  }
  return files;
}

async function api(endpoint, method = 'GET', body, allowMissing = false) {
  const token = process.env.GH_TOKEN;
  if (!token) throw new Error('GH_TOKEN is required.');
  const response = await fetch(`https://api.github.com/repos/${repository}/${endpoint}`, {
    method, headers: { Authorization: `Bearer ${token}`, Accept: 'application/vnd.github+json',
      'X-GitHub-Api-Version': '2022-11-28', ...(body ? { 'Content-Type': 'application/json' } : {}) },
    ...(body ? { body: JSON.stringify(body) } : {}),
  });
  if (response.status === 404 && allowMissing) return null;
  if (!response.ok) throw new Error(`GitHub ${method} ${endpoint}: ${response.status}`);
  return response.json();
}

async function assertForward(release) {
  const latest = await api('releases/latest', 'GET', undefined, true);
  if (latest && latest.tag_name !== release.tag) {
    const previous = latest.tag_name.replace(/^v/, '');
    if (compareVersions(release.version, previous) <= 0) throw new Error('Refusing stale/out-of-order stable promotion.');
  }
}

async function anonymousVerify(release, files, directory) {
  for (const name of files) {
    const response = await fetch(`https://github.com/${repository}/releases/download/${release.tag}/${name}`);
    if (!response.ok || new URL(response.url).protocol !== 'https:') throw new Error(`Anonymous HTTPS download failed: ${name}`);
    const actual = Buffer.from(await response.arrayBuffer());
    if (!actual.equals(await readFile(path.join(directory, name)))) throw new Error(`Anonymous asset bytes differ: ${name}`);
  }
}

async function publish(directory) {
  if (process.env.GITHUB_REF_TYPE !== 'tag' || !process.env.GITHUB_REF_NAME) {
    throw new Error('Stable publication requires a tagged Actions job.');
  }
  const release = await identity(process.env.GITHUB_REF_NAME);
  const files = await verifyArtifacts(directory, release);
  await assertForward(release);
  const remoteTag = await api(`commits/${release.tag}`);
  if (remoteTag.sha !== release.commit) throw new Error('Remote tag no longer matches the built commit.');
  let existing = await api(`releases/tags/${release.tag}`, 'GET', undefined, true);
  const notes = `Listenbox ${release.version}\n\nDeveloper ID signed and notarized macOS Apple Silicon DMG; Windows x64 and native ARM64 per-user installers. Sparkle and WinSparkle verify Ed25519 signatures for in-app updates on all platforms.\n\nWindows installers do not yet have an Authenticode publisher certificate and may show a SmartScreen warning.\n\nSource commit: ${release.commit}.\n\nExisting builds without an updater require manual installation. Portable Windows users: close Listenbox and run the matching Setup EXE; the existing profile is retained.\n`;
  if (!existing) {
    existing = await api('releases', 'POST', { tag_name: release.tag, target_commitish: release.commit,
      name: `Listenbox Desktop ${release.version}`, body: notes, draft: true, prerelease: false, make_latest: 'false' });
  }
  if (existing.target_commitish !== release.commit || existing.prerelease || existing.body !== notes) {
    throw new Error('Existing release has conflicting identity or notes.');
  }
  const staging = await mkdtemp(path.join(tmpdir(), 'listenbox-release-'));
  try {
    for (const asset of existing.assets) {
      if (!files.includes(asset.name)) throw new Error(`Unexpected existing release asset: ${asset.name}`);
      execFileSync('gh', ['release', 'download', release.tag, '--repo', repository, '--pattern', asset.name, '--dir', staging], { stdio: 'pipe' });
      if (asset.size !== (await readFile(path.join(directory, asset.name))).length
          || !(await readFile(path.join(staging, asset.name))).equals(await readFile(path.join(directory, asset.name)))) {
        throw new Error(`Existing immutable asset conflicts: ${asset.name}`);
      }
    }
    for (const name of files) {
      if (existing.assets.some(asset => asset.name === name)) continue;
      if (!existing.draft) throw new Error('Cannot add missing assets to a published release.');
      execFileSync('gh', ['release', 'upload', release.tag, '--repo', repository, path.join(directory, name)], { stdio: 'pipe' });
    }
    await rm(staging, { recursive: true, force: true });
    await mkdir(staging);
    execFileSync('gh', ['release', 'download', release.tag, '--repo', repository, '--dir', staging], { stdio: 'pipe' });
    await verifyArtifacts(staging, release);
    for (const name of files) {
      if (!(await readFile(path.join(staging, name))).equals(await readFile(path.join(directory, name)))) {
        throw new Error(`Uploaded bytes differ: ${name}`);
      }
    }
    existing = await api(`releases/${existing.id}`);
    if (existing.assets.length !== files.length || existing.assets.some(asset => !files.includes(asset.name))) {
      throw new Error('Uploaded asset set is incomplete or unexpected.');
    }
    if (existing.draft) await api(`releases/${existing.id}`, 'PATCH', { draft: false, make_latest: 'false' });
    await anonymousVerify(release, files, directory);
    await assertForward(release);
    await api(`releases/${existing.id}`, 'PATCH', { make_latest: 'true' });
    const promoted = await api('releases/latest');
    if (promoted.tag_name !== release.tag) throw new Error('Latest promotion did not select the verified release.');
    console.log(`Promoted complete stable release ${release.tag}`);
  } finally {
    await rm(staging, { recursive: true, force: true });
  }
}

if (command === 'validate') {
  const release = await identity(args[0]);
  console.log(JSON.stringify(release));
  if (process.env.GITHUB_OUTPUT) await appendFile(process.env.GITHUB_OUTPUT,
    `version=${release.version}\nwindows-version=${release.windowsVersion}\ncommit=${release.commit}\n`);
} else if (command === 'prepare') {
  await prepare(...args);
} else if (command === 'verify') {
  await verifyArtifacts(args[0], await identity(process.env.GITHUB_REF_TYPE === 'tag' ? process.env.GITHUB_REF_NAME : undefined));
} else if (command === 'publish') {
  await publish(args[0]);
} else {
  throw new Error('Usage: release.mjs validate [tag] | prepare PLATFORM ARTIFACT OUTPUT | verify DIRECTORY | publish DIRECTORY');
}
