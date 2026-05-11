#!/usr/bin/env node

"use strict";

const { execFileSync } = require("child_process");
const path = require("path");

const binName = process.platform === "win32" ? "srcwalk.exe" : "srcwalk";
const bin = path.join(__dirname, "bin", binName);

try {
  execFileSync(bin, process.argv.slice(2), { stdio: "inherit" });
} catch (err) {
  if (err.status != null) {
    process.exit(err.status);
  }
  console.error(`srcwalk: failed to run binary at ${bin}`);
  console.error(err.message);
  process.exit(1);
}
