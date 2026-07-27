#!/usr/bin/env node

"use strict";

const crypto = require("crypto");
const fs = require("fs");
const https = require("https");
const path = require("path");
const { execFile } = require("child_process");
const { pipeline } = require("stream");

const PLATFORM_MAP = {
  "linux-x64": { target: "x86_64-unknown-linux-musl", binName: "srcwalk" },
  "linux-arm64": { target: "aarch64-unknown-linux-musl", binName: "srcwalk" },
  "darwin-x64": { target: "x86_64-apple-darwin", binName: "srcwalk" },
  "darwin-arm64": { target: "aarch64-apple-darwin", binName: "srcwalk" },
  "win32-x64": { target: "x86_64-pc-windows-msvc", binName: "srcwalk.exe" },
  // Windows on ARM runs x64 binaries via OS emulation; native ARM64 is not shipped yet.
  "win32-arm64": { target: "x86_64-pc-windows-msvc", binName: "srcwalk.exe" },
};

const MAX_REDIRECTS = 5;
const REQUEST_TIMEOUT_MS = 30_000;
const MAX_CHECKSUM_BYTES = 4 * 1024;
const MAX_ARCHIVE_BYTES = 100 * 1024 * 1024;

function resolveRedirectUrl(currentUrl, location) {
  const next = new URL(location, currentUrl);
  if (next.protocol !== "https:") {
    throw new Error(`refusing non-HTTPS redirect to ${next.href}`);
  }
  return next.href;
}

function getResponse(url, redirects = 0) {
  if (new URL(url).protocol !== "https:") {
    return Promise.reject(new Error(`refusing non-HTTPS download URL ${url}`));
  }

  return new Promise((resolve, reject) => {
    const request = https.get(url, { headers: { "User-Agent": "srcwalk-npm" } }, (response) => {
      const status = response.statusCode || 0;
      if (status >= 300 && status < 400 && response.headers.location) {
        response.resume();
        if (redirects >= MAX_REDIRECTS) {
          reject(new Error(`download exceeded ${MAX_REDIRECTS} redirects`));
          return;
        }
        let nextUrl;
        try {
          nextUrl = resolveRedirectUrl(url, response.headers.location);
        } catch (error) {
          reject(error);
          return;
        }
        resolve(getResponse(nextUrl, redirects + 1));
        return;
      }
      if (status !== 200) {
        response.resume();
        reject(new Error(`download failed (HTTP ${status}) for ${url}`));
        return;
      }
      response.setTimeout(REQUEST_TIMEOUT_MS, () => {
        response.destroy(new Error(`download timed out after ${REQUEST_TIMEOUT_MS / 1000}s`));
      });
      resolve(response);
    });

    request.setTimeout(REQUEST_TIMEOUT_MS, () => {
      request.destroy(new Error(`request timed out after ${REQUEST_TIMEOUT_MS / 1000}s`));
    });
    request.on("error", reject);
  });
}

async function downloadText(url, maxBytes) {
  const response = await getResponse(url);
  return new Promise((resolve, reject) => {
    const chunks = [];
    let size = 0;
    let settled = false;

    function finish(error, value) {
      if (settled) return;
      settled = true;
      if (error) reject(error);
      else resolve(value);
    }

    response.on("data", (chunk) => {
      size += chunk.length;
      if (size > maxBytes) {
        response.destroy();
        finish(new Error(`response exceeds ${maxBytes} bytes`));
        return;
      }
      chunks.push(chunk);
    });
    response.on("end", () => finish(null, Buffer.concat(chunks).toString("utf8")));
    response.on("error", (error) => finish(error));
  });
}

async function downloadArchive(url, destination, maxBytes) {
  const response = await getResponse(url);
  const hash = crypto.createHash("sha256");
  let size = 0;

  response.on("data", (chunk) => {
    size += chunk.length;
    hash.update(chunk);
    if (size > maxBytes) {
      response.destroy(new Error(`archive exceeds ${maxBytes} bytes`));
    }
  });

  await new Promise((resolve, reject) => {
    pipeline(response, fs.createWriteStream(destination, { flags: "wx" }), (error) => {
      if (error) reject(error);
      else resolve();
    });
  });

  return { bytes: size, sha256: hash.digest("hex") };
}

function parseChecksum(text, expectedFilename) {
  const lines = text.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
  if (lines.length !== 1) {
    throw new Error("checksum file must contain exactly one non-empty line");
  }
  const match = lines[0].match(/^([a-f0-9]{64})\s+\*?(.+)$/i);
  if (!match) {
    throw new Error("checksum file has an invalid SHA-256 format");
  }
  const listedFilename = match[2].trim().split(/[\\/]/).pop();
  if (listedFilename !== expectedFilename) {
    throw new Error(`checksum names ${listedFilename || "<empty>"}, expected ${expectedFilename}`);
  }
  return match[1].toLowerCase();
}

function validateArchiveEntries(listing, expectedBinary) {
  const entries = listing.split(/\r?\n/).map((entry) => entry.trim()).filter(Boolean);
  if (entries.length !== 1) {
    throw new Error(`archive must contain exactly one entry, found ${entries.length}`);
  }
  const normalized = entries[0].replace(/\\/g, "/").replace(/^(\.\/)+/, "");
  if (normalized !== expectedBinary) {
    throw new Error(`archive entry ${entries[0]} does not match expected binary ${expectedBinary}`);
  }
}

function runTar(args, cwd) {
  return new Promise((resolve, reject) => {
    execFile("tar", args, { cwd, encoding: "utf8", maxBuffer: 1024 * 1024, windowsHide: true }, (error, stdout) => {
      if (error) {
        const detail = error.code === "ENOENT" ? "tar is required but was not found on PATH" : error.message;
        reject(new Error(`tar ${args[0]} failed: ${detail}`));
      } else {
        resolve(stdout);
      }
    });
  });
}

function removeTree(target) {
  if (!fs.existsSync(target)) return;
  if (typeof fs.rmSync === "function") {
    fs.rmSync(target, { recursive: true, force: true });
  } else {
    fs.rmdirSync(target, { recursive: true });
  }
}

function hashesMatch(actual, expected) {
  if (!/^[a-f0-9]{64}$/i.test(actual) || !/^[a-f0-9]{64}$/i.test(expected)) return false;
  const left = Buffer.from(actual, "hex");
  const right = Buffer.from(expected, "hex");
  return left.length === right.length && crypto.timingSafeEqual(left, right);
}

async function install() {
  const key = `${process.platform}-${process.arch}`;
  const platform = PLATFORM_MAP[key];
  if (!platform) {
    throw new Error(`unsupported platform ${key}. Supported: ${Object.keys(PLATFORM_MAP).join(", ")}`);
  }

  const version = require("./package.json").version;
  const archiveName = `srcwalk-${platform.target}.tar.gz`;
  const archiveUrl = `https://github.com/sting8k/srcwalk/releases/download/v${version}/${archiveName}`;
  const checksumUrl = `${archiveUrl}.sha256`;
  const binDir = path.join(__dirname, "bin");
  const binPath = path.join(binDir, platform.binName);

  if (fs.existsSync(binPath)) {
    const existing = fs.lstatSync(binPath);
    if (!existing.isFile() || existing.isSymbolicLink()) {
      throw new Error(`refusing unexpected existing binary path ${binPath}`);
    }
    return;
  }

  fs.mkdirSync(binDir, { recursive: true });
  const tempRoot = fs.mkdtempSync(path.join(binDir, ".install-"));
  const archivePath = path.join(tempRoot, archiveName);
  const stagingDir = path.join(tempRoot, "staging");
  fs.mkdirSync(stagingDir);

  console.log(`srcwalk: downloading ${platform.target} binary...`);
  try {
    const checksumText = await downloadText(checksumUrl, MAX_CHECKSUM_BYTES);
    const expectedSha256 = parseChecksum(checksumText, archiveName);
    const downloaded = await downloadArchive(archiveUrl, archivePath, MAX_ARCHIVE_BYTES);
    if (!hashesMatch(downloaded.sha256, expectedSha256)) {
      throw new Error(`SHA-256 mismatch for ${archiveName}`);
    }
    console.log(`srcwalk: verified SHA-256 (${downloaded.bytes} bytes)`);

    // Relative tar operands avoid drive-letter paths being parsed as remote
    // archives by GNU tar when npm runs from Git Bash on Windows.
    const listing = await runTar(["tzf", archiveName], tempRoot);
    validateArchiveEntries(listing, platform.binName);
    await runTar(["xzf", archiveName, "-C", "staging"], tempRoot);

    const stagedBinary = path.join(stagingDir, platform.binName);
    const staged = fs.lstatSync(stagedBinary);
    if (!staged.isFile() || staged.isSymbolicLink()) {
      throw new Error(`archive did not produce a regular ${platform.binName} file`);
    }
    if (process.platform !== "win32") {
      fs.chmodSync(stagedBinary, 0o755);
    }
    fs.renameSync(stagedBinary, binPath);
    console.log("srcwalk: installed successfully");
  } finally {
    removeTree(tempRoot);
  }
}

async function main() {
  try {
    await install();
  } catch (error) {
    console.error(`srcwalk: install failed: ${error.message}`);
    console.error("Install manually: cargo install srcwalk --locked");
    process.exitCode = 1;
  }
}

if (require.main === module) {
  main();
}

module.exports = {
  PLATFORM_MAP,
  hashesMatch,
  parseChecksum,
  resolveRedirectUrl,
  runTar,
  validateArchiveEntries,
};
