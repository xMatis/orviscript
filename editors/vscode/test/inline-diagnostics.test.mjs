import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";

const require = createRequire(import.meta.url);
const { inlineDiagnosticEntries } = require("../inline-diagnostics.js");

const diagnostic = {
  code: "ORVI1001",
  message: "Unknown syntax. Did you mean 'fn'?",
  range: { start: { line: 3 }, end: { line: 3 } },
};

test("builds one inline entry per line for OrviScript rule diagnostics", () => {
  assert.deepEqual(inlineDiagnosticEntries([diagnostic, diagnostic], true), [{
    line: 3,
    text: "ORVI1001: Unknown syntax. Did you mean 'fn'?",
  }]);
});

test("disables inline entries without hiding standard diagnostics", () => {
  assert.deepEqual(inlineDiagnosticEntries([diagnostic], false), []);
});
