// Copies the compiled Rust library to ./triplelight.node, the file index.js loads.
const fs = require("node:fs");
const path = require("node:path");
const profile = process.argv[2] || "release";
const target = process.env.CARGO_TARGET_DIR || path.join(__dirname, "..", "target");
const names = { darwin: "libtriplelight_node.dylib", linux: "libtriplelight_node.so" };
const name = names[process.platform];
if (!name) throw new Error(`unsupported platform ${process.platform}: TripleLight supports Linux and macOS`);
fs.copyFileSync(path.join(target, profile, name), path.join(__dirname, "..", "triplelight.node"));
