const fs = require("node:fs");
const path = require("node:path");

function resolveServer(options) {
  const {
    configured = "",
    extensionPath,
    platform = process.platform,
    env = process.env,
    existsSync = fs.existsSync,
  } = options;
  if (configured) return configured;

  const executable = platform === "win32" ? "orvi-lsp.exe" : "orvi-lsp";
  const candidates = [path.resolve(extensionPath, "..", "..", "target", "debug", executable)];
  if (platform === "win32") {
    if (env.LOCALAPPDATA) {
      candidates.push(path.join(env.LOCALAPPDATA, "Programs", "OrviScript", executable));
    }
  } else {
    if (env.HOME) candidates.push(path.join(env.HOME, ".local", "bin", executable));
    candidates.push(path.join("/usr/local/bin", executable), path.join("/usr/bin", executable));
  }

  return candidates.find((candidate) => existsSync(candidate)) ?? executable;
}

module.exports = { resolveServer };
