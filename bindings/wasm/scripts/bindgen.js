// Generates the JS glue: pkg/ for browsers and bundlers (ES module, `await init()` first), pkg-node/ for Node tests.
// Needs wasm-bindgen-cli matching the crate's pinned wasm-bindgen: `cargo install wasm-bindgen-cli --version 0.2.100`.
const { execFileSync } = require("node:child_process");
const path = require("node:path");
const target = process.env.CARGO_TARGET_DIR || path.join(__dirname, "..", "target");
const wasm = path.join(target, "wasm32-unknown-unknown", "release", "triplelight_wasm.wasm");
for (const [kind, dir] of [["web", "pkg"], ["nodejs", "pkg-node"]]) {
  execFileSync("wasm-bindgen", ["--target", kind, "--out-dir", path.join(__dirname, "..", dir), wasm], { stdio: "inherit" });
}
