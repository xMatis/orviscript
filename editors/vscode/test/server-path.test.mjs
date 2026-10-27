import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const { resolveServer } = require("../server-path.js");

test("prefers an explicitly configured language server", () => {
  assert.equal(resolveServer({
    configured: "D:\\tools\\orvi-lsp.exe",
    extensionPath: "D:\\extension",
    platform: "win32",
    env: {},
    existsSync: () => false,
  }), "D:\\tools\\orvi-lsp.exe");
});

test("finds the Windows installer language server without a refreshed PATH", () => {
  const localAppData = path.join("C:\\Users\\developer", "AppData", "Local");
  const installed = path.join(localAppData, "Programs", "OrviScript", "orvi-lsp.exe");
  assert.equal(resolveServer({
    extensionPath: "C:\\extension",
    platform: "win32",
    env: { LOCALAPPDATA: localAppData },
    existsSync: (candidate) => candidate === installed,
  }), installed);
});

test("falls back to PATH when no known installation exists", () => {
  assert.equal(resolveServer({
    extensionPath: "/extension",
    platform: "linux",
    env: {},
    existsSync: () => false,
  }), "orvi-lsp");
});
