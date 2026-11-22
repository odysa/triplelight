// Runs before `npm pack` / `npm publish`: the package must carry an addon for every supported platform.
const fs = require("node:fs");
const path = require("node:path");
const targets = ["darwin-arm64", "linux-x64", "linux-arm64"];
const missing = targets.filter((t) => !fs.existsSync(path.join(__dirname, "..", "prebuilds", t, "triplelight.node")));
if (missing.length) {
  console.error(`prebuilds missing for: ${missing.join(", ")}. Run \`npm run build:prebuilds\` (and see scripts/build-prebuilds.sh).`);
  process.exit(1);
}
