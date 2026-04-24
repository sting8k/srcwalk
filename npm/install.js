#!/usr/bin/env node

"use strict";

const https = require("https");
const http = require("http");
const fs = require("fs");
const path = require("path");
const zlib = require("zlib");

const PLATFORM_MAP = {
  "linux-x64": "x86_64-unknown-linux-musl",
  "linux-arm64": "aarch64-unknown-linux-musl",
  "darwin-x64": "x86_64-apple-darwin",
  "darwin-arm64": "aarch64-apple-darwin",
};

const key = `${process.platform}-${process.arch}`;
const target = PLATFORM_MAP[key];

if (!target) {
  console.error(`srcwalk: unsupported platform ${key}`);
  console.error(`Supported: ${Object.keys(PLATFORM_MAP).join(", ")}`);
  process.exit(1);
}

const version = require("./package.json").version;
const url = `https://github.com/sting8k/srcwalk/releases/download/v${version}/srcwalk-${target}.tar.gz`;
const binName = "srcwalk";

const binDir = path.join(__dirname, "bin");
const binPath = path.join(binDir, binName);

// Skip if binary already exists (e.g. re-install)
if (fs.existsSync(binPath)) {
  process.exit(0);
}

fs.mkdirSync(binDir, { recursive: true });

console.log(`srcwalk: downloading ${target} binary...`);

function follow(url, callback) {
  const mod = url.startsWith("https") ? https : http;
  mod.get(url, { headers: { "User-Agent": "srcwalk-npm" } }, (res) => {
    if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
      follow(res.headers.location, callback);
    } else if (res.statusCode !== 200) {
      console.error(`srcwalk: download failed (HTTP ${res.statusCode})`);
      console.error(`URL: ${url}`);
      console.error("Install manually: cargo install srcwalk");
      process.exit(1);
    } else {
      callback(res);
    }
  }).on("error", (err) => {
    console.error(`srcwalk: download failed: ${err.message}`);
    console.error("Install manually: cargo install srcwalk");
    process.exit(1);
  });
}

follow(url, (res) => {
  const tar = require("child_process").spawn("tar", ["xz", "-C", binDir], {
    stdio: ["pipe", "inherit", "inherit"],
  });
  res.pipe(tar.stdin);
  tar.on("close", (code) => {
    if (code !== 0) {
      console.error("srcwalk: failed to extract. Install manually: cargo install srcwalk");
      process.exit(1);
    }
    fs.chmodSync(binPath, 0o755);
    console.log("srcwalk: installed successfully");
  });
});
