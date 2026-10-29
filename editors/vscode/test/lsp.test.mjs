import assert from "node:assert/strict";
import childProcess from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import url from "node:url";

import { MessageReader, encodeMessage } from "../protocol.mjs";

const extensionRoot = path.dirname(path.dirname(url.fileURLToPath(import.meta.url)));
const repositoryRoot = path.resolve(extensionRoot, "..", "..");
const executable = path.join(
  repositoryRoot,
  "target",
  "debug",
  process.platform === "win32" ? "orvi-lsp.exe" : "orvi-lsp",
);

test("language server publishes diagnostics for an open document", { timeout: 60000 }, async () => {
  const server = childProcess.spawn(executable, [], { stdio: ["pipe", "pipe", "pipe"] });
  const exited = new Promise((resolve) => server.once("exit", resolve));
  const reader = new MessageReader();
  const received = [];
  let wake;

  server.stdout.on("data", (chunk) => {
    received.push(...reader.append(chunk));
    wake?.();
  });

  const send = (message) => server.stdin.write(encodeMessage(message));
  const waitFor = async (predicate) => {
    while (true) {
      const index = received.findIndex(predicate);
      if (index >= 0) return received.splice(index, 1)[0];
      await new Promise((resolve) => { wake = resolve; });
      wake = undefined;
    }
  };

  try {
    send({
      jsonrpc: "2.0",
      id: 1,
      method: "initialize",
      params: { processId: process.pid, rootUri: null, capabilities: {} },
    });
    const initialized = await waitFor((message) => message.id === 1);
    assert.equal(initialized.result.serverInfo.name, "orvi-lsp");
    assert.equal(initialized.result.serverInfo.version, "0.10.1");
    assert.equal(initialized.result.capabilities.textDocumentSync, 1);

    send({ jsonrpc: "2.0", method: "initialized", params: {} });
    send({
      jsonrpc: "2.0",
      method: "textDocument/didOpen",
      params: {
        textDocument: {
          uri: "file:///diagnostic.orvi",
          languageId: "orviscript",
          version: 1,
          text: "let answer = ;",
        },
      },
    });

    const publication = await waitFor(
      (message) => message.method === "textDocument/publishDiagnostics",
    );
    assert.equal(publication.params.diagnostics.length, 1);
    assert.match(publication.params.diagnostics[0].message, /expected expression/);
    assert.equal(publication.params.diagnostics[0].source, "orviscript");

    send({
      jsonrpc: "2.0",
      method: "textDocument/didChange",
      params: {
        textDocument: { uri: "file:///diagnostic.orvi", version: 2 },
        contentChanges: [{ text: "let answer = 42; print(answer);" }],
      },
    });
    const corrected = await waitFor(
      (message) => message.method === "textDocument/publishDiagnostics"
        && message.params.version === 2,
    );
    assert.deepEqual(corrected.params.diagnostics, []);

    const foreignUri = "file:///foreign-syntax.orvi";
    send({
      jsonrpc: "2.0",
      method: "textDocument/didOpen",
      params: {
        textDocument: {
          uri: foreignUri,
          languageId: "orviscript",
          version: 1,
          text: "function greet() {}",
        },
      },
    });
    const foreignDiagnostic = await waitFor(
      (message) => message.method === "textDocument/publishDiagnostics"
        && message.params.uri === foreignUri,
    );
    assert.equal(foreignDiagnostic.params.diagnostics.length, 1);
    assert.equal(foreignDiagnostic.params.diagnostics[0].code, "ORVI1001");
    assert.equal(foreignDiagnostic.params.diagnostics[0].range.end.character, 8);

    send({
      jsonrpc: "2.0",
      id: 30,
      method: "textDocument/codeAction",
      params: {
        textDocument: { uri: foreignUri },
        range: { start: { line: 0, character: 0 }, end: { line: 0, character: 8 } },
        context: { diagnostics: foreignDiagnostic.params.diagnostics },
      },
    });
    const quickFix = await waitFor((message) => message.id === 30);
    assert.equal(quickFix.result.length, 1);
    assert.equal(quickFix.result[0].kind, "quickfix");
    assert.equal(quickFix.result[0].edit.changes[foreignUri][0].newText, "fn");

    send({
      jsonrpc: "2.0",
      method: "workspace/didChangeConfiguration",
      params: {
        settings: {
          diagnostics: {
            enabledCategories: ["another_category"],
            minimumSeverity: "hint",
          },
        },
      },
    });
    const filteredDiagnostic = await waitFor(
      (message) => message.method === "textDocument/publishDiagnostics"
        && message.params.uri === foreignUri && !Object.hasOwn(message.params, "version"),
    );
    assert.deepEqual(filteredDiagnostic.params.diagnostics, []);

    send({
      jsonrpc: "2.0",
      method: "workspace/didChangeConfiguration",
      params: {
        settings: {
          diagnostics: { enabledCategories: [], minimumSeverity: "hint" },
        },
      },
    });
    const restoredDiagnostic = await waitFor(
      (message) => message.method === "textDocument/publishDiagnostics"
        && message.params.uri === foreignUri
        && message.params.diagnostics[0]?.code === "ORVI1001",
    );
    assert.equal(restoredDiagnostic.params.diagnostics.length, 1);

    send({
      jsonrpc: "2.0",
      method: "textDocument/didClose",
      params: { textDocument: { uri: foreignUri } },
    });
    await waitFor((message) => message.method === "textDocument/publishDiagnostics"
      && message.params.uri === foreignUri && !Object.hasOwn(message.params, "version"));

    send({
      jsonrpc: "2.0",
      id: 2,
      method: "textDocument/completion",
      params: {
        textDocument: { uri: "file:///diagnostic.orvi" },
        position: { line: 0, character: 0 },
      },
    });
    const completion = await waitFor((message) => message.id === 2);
    assert.ok(completion.result.some((item) => item.label === "print"));

    const completionUri = "file:///standard-library-completion.orvi";
    send({
      jsonrpc: "2.0",
      method: "textDocument/didOpen",
      params: {
        textDocument: {
          uri: completionUri,
          languageId: "orviscript",
          version: 1,
          text: "import { read } from \"std:fs\";",
        },
      },
    });
    await waitFor((message) => message.method === "textDocument/publishDiagnostics"
      && message.params.uri === completionUri);
    send({
      jsonrpc: "2.0",
      id: 20,
      method: "textDocument/completion",
      params: {
        textDocument: { uri: completionUri },
        position: { line: 0, character: 12 },
      },
    });
    const standardMembers = await waitFor((message) => message.id === 20);
    assert.ok(standardMembers.result.some((item) => item.label === "read_text"));
    assert.ok(!standardMembers.result.some((item) => item.label === "sleep_async"));

    send({
      jsonrpc: "2.0",
      method: "textDocument/didChange",
      params: {
        textDocument: { uri: completionUri, version: 2 },
        contentChanges: [{ text: "fs.re" }],
      },
    });
    await waitFor((message) => message.method === "textDocument/publishDiagnostics"
      && message.params.uri === completionUri && message.params.version === 2);
    send({
      jsonrpc: "2.0",
      id: 21,
      method: "textDocument/completion",
      params: {
        textDocument: { uri: completionUri },
        position: { line: 0, character: 5 },
      },
    });
    const memberCalls = await waitFor((message) => message.id === 21);
    const readText = memberCalls.result.find((item) => item.label === "read_text");
    assert.equal(readText.insertText, "read_text(${1:path})");
    assert.equal(readText.insertTextFormat, 2);

    send({
      jsonrpc: "2.0",
      method: "textDocument/didChange",
      params: {
        textDocument: { uri: completionUri, version: 3 },
        contentChanges: [{ text: "import { value } from \"std:f" }],
      },
    });
    await waitFor((message) => message.method === "textDocument/publishDiagnostics"
      && message.params.uri === completionUri && message.params.version === 3);
    send({
      jsonrpc: "2.0",
      id: 22,
      method: "textDocument/completion",
      params: {
        textDocument: { uri: completionUri },
        position: { line: 0, character: 28 },
      },
    });
    const standardModules = await waitFor((message) => message.id === 22);
    assert.ok(standardModules.result.some((item) => item.label === "std:fs"));

    send({
      jsonrpc: "2.0",
      id: 3,
      method: "textDocument/hover",
      params: {
        textDocument: { uri: "file:///diagnostic.orvi" },
        position: { line: 0, character: 19 },
      },
    });
    const hover = await waitFor((message) => message.id === 3);
    assert.match(hover.result.contents.value, /standard output/);

    send({
      jsonrpc: "2.0",
      id: 4,
      method: "textDocument/semanticTokens/full",
      params: { textDocument: { uri: "file:///diagnostic.orvi" } },
    });
    const semanticTokens = await waitFor((message) => message.id === 4);
    assert.ok(semanticTokens.result.data.length > 0);

    send({
      jsonrpc: "2.0",
      id: 5,
      method: "textDocument/definition",
      params: {
        textDocument: { uri: "file:///diagnostic.orvi" },
        position: { line: 0, character: 25 },
      },
    });
    const definition = await waitFor((message) => message.id === 5);
    assert.equal(definition.result.range.start.character, 4);

    send({
      jsonrpc: "2.0",
      id: 6,
      method: "textDocument/references",
      params: {
        textDocument: { uri: "file:///diagnostic.orvi" },
        position: { line: 0, character: 25 },
        context: { includeDeclaration: true },
      },
    });
    const references = await waitFor((message) => message.id === 6);
    assert.equal(references.result.length, 2);

    send({
      jsonrpc: "2.0",
      id: 7,
      method: "textDocument/prepareRename",
      params: {
        textDocument: { uri: "file:///diagnostic.orvi" },
        position: { line: 0, character: 25 },
      },
    });
    const preparation = await waitFor((message) => message.id === 7);
    assert.equal(preparation.result.placeholder, "answer");

    send({
      jsonrpc: "2.0",
      id: 8,
      method: "textDocument/rename",
      params: {
        textDocument: { uri: "file:///diagnostic.orvi" },
        position: { line: 0, character: 25 },
        newName: "result",
      },
    });
    const rename = await waitFor((message) => message.id === 8);
    assert.equal(rename.result.changes["file:///diagnostic.orvi"].length, 2);

    send({
      jsonrpc: "2.0",
      method: "textDocument/didChange",
      params: {
        textDocument: { uri: "file:///diagnostic.orvi", version: 3 },
        contentChanges: [{ text: "let answer=42;print(answer)" }],
      },
    });
    await waitFor(
      (message) => message.method === "textDocument/publishDiagnostics"
        && message.params.version === 3,
    );
    send({
      jsonrpc: "2.0",
      id: 9,
      method: "textDocument/formatting",
      params: {
        textDocument: { uri: "file:///diagnostic.orvi" },
        options: { tabSize: 4, insertSpaces: true },
      },
    });
    const formatting = await waitFor((message) => message.id === 9);
    assert.equal(formatting.result[0].newText, "let answer = 42;\nprint(answer)\n");

    const project = fs.mkdtempSync(path.join(os.tmpdir(), "orviscript-lsp-"));
    const sourceDirectory = path.join(project, "src");
    fs.mkdirSync(sourceDirectory);
    fs.writeFileSync(
      path.join(project, "orvi.toml"),
      "[project]\nname = \"lsp-test\"\nversion = \"0.1.0\"\nentry = \"src/main.orvi\"\n",
    );
    const modulePath = path.join(sourceDirectory, "messages.orvi");
    const entryPath = path.join(sourceDirectory, "main.orvi");
    const moduleSource = "export fn greet(name: string) -> string { return name; }";
    const entrySource = "import { greet } from \"./messages.orvi\"; print(greet(\"Ada\"));";
    fs.writeFileSync(modulePath, moduleSource);
    fs.writeFileSync(entryPath, entrySource);
    const moduleUri = url.pathToFileURL(modulePath).href;
    const entryUri = url.pathToFileURL(entryPath).href;
    const importedUse = entrySource.lastIndexOf("greet") + 1;

    send({
      jsonrpc: "2.0",
      method: "textDocument/didOpen",
      params: {
        textDocument: {
          uri: entryUri,
          languageId: "orviscript",
          version: 1,
          text: entrySource,
        },
      },
    });
    const projectDiagnostics = await waitFor(
      (message) => message.method === "textDocument/publishDiagnostics"
        && message.params.uri === entryUri,
    );
    assert.deepEqual(projectDiagnostics.params.diagnostics, []);

    send({
      jsonrpc: "2.0",
      id: 10,
      method: "textDocument/definition",
      params: {
        textDocument: { uri: entryUri },
        position: { line: 0, character: importedUse },
      },
    });
    const projectDefinition = await waitFor((message) => message.id === 10);
    assert.equal(decodeURIComponent(projectDefinition.result.uri), decodeURIComponent(moduleUri));
    assert.equal(projectDefinition.result.range.start.character, 10);

    send({
      jsonrpc: "2.0",
      id: 11,
      method: "textDocument/references",
      params: {
        textDocument: { uri: entryUri },
        position: { line: 0, character: importedUse },
        context: { includeDeclaration: true },
      },
    });
    const projectReferences = await waitFor((message) => message.id === 11);
    assert.equal(projectReferences.result.length, 3);

    send({
      jsonrpc: "2.0",
      id: 12,
      method: "textDocument/rename",
      params: {
        textDocument: { uri: entryUri },
        position: { line: 0, character: importedUse },
        newName: "welcome",
      },
    });
    const projectRename = await waitFor((message) => message.id === 12);
    const moduleChanges = Object.entries(projectRename.result.changes)
      .find(([uri]) => decodeURIComponent(uri) === decodeURIComponent(moduleUri))?.[1];
    const entryChanges = Object.entries(projectRename.result.changes)
      .find(([uri]) => decodeURIComponent(uri) === decodeURIComponent(entryUri))?.[1];
    assert.equal(moduleChanges.length, 1);
    assert.equal(entryChanges.length, 2);

    send({
      jsonrpc: "2.0",
      method: "textDocument/didOpen",
      params: {
        textDocument: {
          uri: moduleUri,
          languageId: "orviscript",
          version: 1,
          text: moduleSource,
        },
      },
    });
    await waitFor(
      (message) => message.method === "textDocument/publishDiagnostics"
        && decodeURIComponent(message.params.uri) === decodeURIComponent(moduleUri),
    );
    send({
      jsonrpc: "2.0",
      method: "textDocument/didChange",
      params: {
        textDocument: { uri: moduleUri, version: 2 },
        contentChanges: [{ text: moduleSource.replace("export ", "") }],
      },
    });
    const invalidatedImporter = await waitFor(
      (message) => message.method === "textDocument/publishDiagnostics"
        && decodeURIComponent(message.params.uri) === decodeURIComponent(entryUri)
        && message.params.diagnostics.some((item) => /has no export 'greet'/.test(item.message)),
    );
    assert.equal(invalidatedImporter.params.diagnostics.length, 1);
    send({
      jsonrpc: "2.0",
      method: "textDocument/didChange",
      params: {
        textDocument: { uri: moduleUri, version: 3 },
        contentChanges: [{ text: moduleSource }],
      },
    });
    const restoredImporter = await waitFor(
      (message) => message.method === "textDocument/publishDiagnostics"
        && decodeURIComponent(message.params.uri) === decodeURIComponent(entryUri)
        && !Object.hasOwn(message.params, "version"),
    );
    assert.deepEqual(restoredImporter.params.diagnostics, []);
    send({
      jsonrpc: "2.0",
      method: "textDocument/didClose",
      params: { textDocument: { uri: moduleUri } },
    });

    send({
      jsonrpc: "2.0",
      method: "textDocument/didClose",
      params: { textDocument: { uri: entryUri } },
    });
    fs.rmSync(project, { recursive: true, force: true });

    send({
      jsonrpc: "2.0",
      method: "textDocument/didClose",
      params: { textDocument: { uri: "file:///diagnostic.orvi" } },
    });
    const closed = await waitFor(
      (message) => message.method === "textDocument/publishDiagnostics"
        && message.params.uri === "file:///diagnostic.orvi"
        && !Object.hasOwn(message.params, "version"),
    );
    assert.deepEqual(closed.params.diagnostics, []);

    send({
      jsonrpc: "2.0",
      method: "textDocument/didClose",
      params: { textDocument: { uri: completionUri } },
    });
    await waitFor((message) => message.method === "textDocument/publishDiagnostics"
      && message.params.uri === completionUri && !Object.hasOwn(message.params, "version"));

    send({ jsonrpc: "2.0", id: 13, method: "shutdown", params: null });
    await waitFor((message) => message.id === 13);
    send({ jsonrpc: "2.0", method: "exit", params: null });
    server.stdin.end();
    await Promise.race([exited, new Promise((resolve) => setTimeout(resolve, 2000))]);
  } finally {
    if (!server.stdin.destroyed) server.stdin.end();
    if (server.exitCode === null) server.kill();
  }
});
