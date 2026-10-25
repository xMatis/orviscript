import fs from "node:fs";
import path from "node:path";
import url from "node:url";

const root = path.dirname(url.fileURLToPath(import.meta.url));
const readJson = (relativePath) => JSON.parse(fs.readFileSync(path.join(root, relativePath), "utf8"));
const assert = (condition, message) => {
  if (!condition) throw new Error(message);
};

const manifest = readJson("package.json");
assert(manifest.main === "./extension.js", "missing extension entry point");
assert(fs.existsSync(path.join(root, manifest.main)), "missing extension entry point file");
assert(
  manifest.activationEvents.includes("onLanguage:orviscript"),
  "missing OrviScript activation event",
);
const language = manifest.contributes.languages.find(({ id }) => id === "orviscript");
assert(language, "missing OrviScript language contribution");
assert(language.extensions.includes(".orvi"), "missing .orvi file extension");
assert(fs.existsSync(path.join(root, language.configuration)), "missing language configuration");
assert(language.icon?.light && language.icon?.dark, "missing OrviScript file icons");
assert(fs.existsSync(path.join(root, language.icon.light)), "missing light file icon");
assert(fs.existsSync(path.join(root, language.icon.dark)), "missing dark file icon");

const grammarContribution = manifest.contributes.grammars.find(({ language: id }) => id === "orviscript");
assert(grammarContribution, "missing OrviScript grammar contribution");
const grammar = readJson(grammarContribution.path);
assert(grammar.scopeName === grammarContribution.scopeName, "grammar scope does not match manifest");
assert(Array.isArray(grammar.patterns) && grammar.patterns.length > 0, "grammar has no patterns");
const keywordPatterns = grammar.repository.keywords.patterns.map((pattern) => pattern.match).join(" ");
for (const unsupported of ["async", "await", "parallel", "task"]) {
  assert(!keywordPatterns.includes(unsupported), `grammar advertises unsupported keyword '${unsupported}'`);
}

const configuration = readJson(language.configuration);
assert(configuration.brackets.length === 3, "expected three bracket pairs");
assert(configuration.autoClosingPairs.length >= 4, "expected bracket and quote auto-closing pairs");
assert(
  manifest.contributes.configuration.properties["orviscript.server.path"],
  "missing language server path setting",
);

console.log("OrviScript VS Code extension configuration is valid.");
