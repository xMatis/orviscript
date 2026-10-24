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

test("language server publishes diagnostics for an open document", { timeout: 5000 }, async () => {
  const server = childProcess.spawn(executable, [], { stdio: ["pipe", "pipe", "pipe"] });
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
        contentChanges: [{ text: "let answer = 42;" }],
      },
    });
    const corrected = await waitFor(
      (message) => message.method === "textDocument/publishDiagnostics"
        && message.params.version === 2,
    );
    assert.deepEqual(corrected.params.diagnostics, []);

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

    send({ jsonrpc: "2.0", id: 2, method: "shutdown", params: null });
    await waitFor((message) => message.id === 2);
    send({ jsonrpc: "2.0", method: "exit", params: null });
  } finally {
    server.stdin.end();
    server.kill();
  }
});
