#!/usr/bin/env node

"use strict";

const assert = require("assert");
const fs = require("fs");
const os = require("os");
const path = require("path");
const {
  PLATFORM_MAP,
  hashesMatch,
  parseChecksum,
  resolveRedirectUrl,
  runTar,
  validateArchiveEntries,
} = require("./install.js");

const HASH = "41bc5152f4f47ee470c81a32366fe5fd34f9b87d02716b57bb542bd099dc12bf";
const ARCHIVE = "srcwalk-x86_64-pc-windows-msvc.tar.gz";

assert.strictEqual(parseChecksum(`${HASH} *${ARCHIVE}\n`, ARCHIVE), HASH);
assert.strictEqual(parseChecksum(`${HASH.toUpperCase()}  dist/${ARCHIVE}\r\n`, ARCHIVE), HASH);
assert.throws(() => parseChecksum(`${HASH}  wrong.tar.gz\n`, ARCHIVE), /expected/);
assert.throws(() => parseChecksum(`${HASH}\n`, ARCHIVE), /invalid SHA-256 format/);
assert.throws(() => parseChecksum(`${HASH}  ${ARCHIVE}\n${HASH}  ${ARCHIVE}\n`, ARCHIVE), /exactly one/);

assert.strictEqual(
  resolveRedirectUrl("https://github.com/sting8k/srcwalk/releases", "/assets/srcwalk.tar.gz"),
  "https://github.com/assets/srcwalk.tar.gz",
);
assert.strictEqual(
  resolveRedirectUrl("https://github.com/", "https://release-assets.githubusercontent.com/srcwalk.tar.gz"),
  "https://release-assets.githubusercontent.com/srcwalk.tar.gz",
);
assert.throws(() => resolveRedirectUrl("https://github.com/", "http://example.test/srcwalk.tar.gz"), /non-HTTPS/);

assert.doesNotThrow(() => validateArchiveEntries("srcwalk\n", "srcwalk"));
assert.doesNotThrow(() => validateArchiveEntries("./srcwalk.exe\r\n", "srcwalk.exe"));
assert.throws(() => validateArchiveEntries("../srcwalk\n", "srcwalk"), /does not match/);
assert.throws(() => validateArchiveEntries("srcwalk\nREADME.md\n", "srcwalk"), /exactly one/);
assert.throws(() => validateArchiveEntries("bin/srcwalk\n", "srcwalk"), /does not match/);

assert.strictEqual(hashesMatch(HASH, HASH), true);
assert.strictEqual(hashesMatch(HASH, `0${HASH.slice(1)}`), false);
assert.strictEqual(hashesMatch(HASH, HASH.slice(2)), false);
assert.strictEqual(hashesMatch("not-a-hash", "not-a-hash"), false);

for (const key of ["linux-x64", "linux-arm64", "darwin-x64", "darwin-arm64", "win32-x64", "win32-arm64"]) {
  assert.ok(PLATFORM_MAP[key], `missing platform mapping for ${key}`);
}
assert.strictEqual(PLATFORM_MAP["win32-arm64"].target, PLATFORM_MAP["win32-x64"].target);

async function testTarRoundTrip() {
  const originalPath = process.env.PATH;
  process.env.PATH = "";
  try {
    await assert.rejects(runTar(["--version"], process.cwd()), /tar is required but was not found on PATH/);
  } finally {
    process.env.PATH = originalPath;
  }

  const root = fs.mkdtempSync(path.join(os.tmpdir(), "srcwalk-install-test-"));
  try {
    const expected = "fixture-binary";
    fs.writeFileSync(path.join(root, "srcwalk.exe"), expected);
    fs.mkdirSync(path.join(root, "staging"));
    await runTar(["czf", "fixture.tar.gz", "srcwalk.exe"], root);
    const listing = await runTar(["tzf", "fixture.tar.gz"], root);
    validateArchiveEntries(listing, "srcwalk.exe");
    await runTar(["xzf", "fixture.tar.gz", "-C", "staging"], root);
    assert.strictEqual(fs.readFileSync(path.join(root, "staging", "srcwalk.exe"), "utf8"), expected);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
}

testTarRoundTrip()
  .then(() => console.log("PASS: npm installer checksum, redirect, archive, tar, and platform guards"))
  .catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
