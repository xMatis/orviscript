import assert from "node:assert/strict";
import childProcess from "node:child_process";
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
      const message = received.find(predicate);
      if (message) return message;
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
    assert.equal(initialized.result.serverInfo.version, "0.2.0");
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

    send({
      jsonrpc: "2.0",
      method: "textDocument/didClose",
      params: { textDocument: { uri: "file:///diagnostic.orvi" } },
    });
    const closed = await waitFor(
      (message) => message.method === "textDocument/publishDiagnostics"
        && !Object.hasOwn(message.params, "version"),
    );
    assert.deepEqual(closed.params.diagnostics, []);

    send({ jsonrpc: "2.0", id: 10, method: "shutdown", params: null });
    await waitFor((message) => message.id === 10);
    send({ jsonrpc: "2.0", method: "exit", params: null });
    server.stdin.end();
    await Promise.race([exited, new Promise((resolve) => setTimeout(resolve, 2000))]);
  } finally {
    if (!server.stdin.destroyed) server.stdin.end();
    if (server.exitCode === null) server.kill();
  }
});
