// Runs before `npm pack` / `npm publish`: pkg/ (browsers, bundlers) and pkg-node/ (Node) must be built.
const fs = require("node:fs");
const path = require("node:path");
const missing = ["pkg/triplelight_wasm.js", "pkg/triplelight_wasm_bg.wasm", "pkg-node/triplelight_wasm.js"]
  .filter((f) => !fs.existsSync(path.join(__dirname, "..", f)));
if (missing.length) {
  console.error(`not built: ${missing.join(", ")}. Run \`npm run build\` first.`);
  process.exit(1);
}
