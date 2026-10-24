const childProcess = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");
const vscode = require("vscode");

let connection;

async function activate(context) {
  const diagnostics = vscode.languages.createDiagnosticCollection("orviscript");
  const output = vscode.window.createOutputChannel("OrviScript");
  context.subscriptions.push(diagnostics, output);

  try {
    const protocol = await import("./protocol.mjs");
    connection = new LanguageServerConnection(context, diagnostics, output, protocol);
    context.subscriptions.push(connection);
    await connection.start();
  } catch (error) {
    const message = `Could not start OrviScript language server: ${error.message}`;
    output.appendLine(message);
    vscode.window.showErrorMessage(message);
  }
}

function deactivate() {
  return connection?.stop();
}

class LanguageServerConnection {
  constructor(context, diagnostics, output, protocol) {
    this.context = context;
    this.diagnostics = diagnostics;
    this.output = output;
    this.protocol = protocol;
    this.reader = new protocol.MessageReader();
    this.sequence = 0;
    this.pending = new Map();
    this.ready = false;
    this.disposables = [];
  }

  async start() {
    const executable = resolveServer(this.context.extensionPath);
    this.output.appendLine(`Starting ${executable}`);
    this.process = childProcess.spawn(executable, [], {
      cwd: vscode.workspace.workspaceFolders?.[0]?.uri.fsPath,
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true,
    });

    this.process.stdout.on("data", (chunk) => {
      try {
        for (const message of this.reader.append(chunk)) this.handleMessage(message);
      } catch (error) {
        this.output.appendLine(`Protocol error: ${error.message}`);
      }
    });
    this.process.stderr.on("data", (chunk) => this.output.append(chunk.toString("utf8")));
    this.process.on("error", (error) => {
      this.output.appendLine(`Server error: ${error.message}`);
      for (const { reject } of this.pending.values()) reject(error);
      this.pending.clear();
    });
    this.process.on("exit", (code, signal) => {
      this.ready = false;
      this.output.appendLine(`Language server stopped (code ${code}, signal ${signal ?? "none"})`);
    });

    await this.request("initialize", {
      processId: process.pid,
      clientInfo: { name: "OrviScript VS Code", version: "0.1.0" },
      rootUri: vscode.workspace.workspaceFolders?.[0]?.uri.toString() ?? null,
      capabilities: {
        general: { positionEncodings: ["utf-16"] },
        textDocument: { publishDiagnostics: { versionSupport: true } },
      },
    });
    this.notify("initialized", {});
    this.ready = true;

    for (const document of vscode.workspace.textDocuments) this.open(document);
    this.disposables.push(
      vscode.workspace.onDidOpenTextDocument((document) => this.open(document)),
      vscode.workspace.onDidChangeTextDocument((event) => this.change(event.document)),
      vscode.workspace.onDidCloseTextDocument((document) => this.close(document)),
    );
  }

  open(document) {
    if (!this.ready || document.languageId !== "orviscript") return;
    this.notify("textDocument/didOpen", {
      textDocument: {
        uri: document.uri.toString(),
        languageId: "orviscript",
        version: document.version,
        text: document.getText(),
      },
    });
  }

  change(document) {
    if (!this.ready || document.languageId !== "orviscript") return;
    this.notify("textDocument/didChange", {
      textDocument: { uri: document.uri.toString(), version: document.version },
      contentChanges: [{ text: document.getText() }],
    });
  }

  close(document) {
    if (!this.ready || document.languageId !== "orviscript") return;
    this.notify("textDocument/didClose", {
      textDocument: { uri: document.uri.toString() },
    });
    this.diagnostics.delete(document.uri);
  }

  handleMessage(message) {
    if (Object.hasOwn(message, "id") && (Object.hasOwn(message, "result") || message.error)) {
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      if (message.error) pending.reject(new Error(message.error.message));
      else pending.resolve(message.result);
      return;
    }

    if (message.method === "textDocument/publishDiagnostics") {
      const uri = vscode.Uri.parse(message.params.uri);
      const items = message.params.diagnostics.map((item) => {
        const range = new vscode.Range(
          item.range.start.line,
          item.range.start.character,
          item.range.end.line,
          item.range.end.character,
        );
        const severity = [undefined, vscode.DiagnosticSeverity.Error, vscode.DiagnosticSeverity.Warning,
          vscode.DiagnosticSeverity.Information, vscode.DiagnosticSeverity.Hint][item.severity];
        const diagnostic = new vscode.Diagnostic(range, item.message, severity);
        diagnostic.source = item.source;
        diagnostic.code = item.code;
        return diagnostic;
      });
      this.diagnostics.set(uri, items);
      return;
    }

    if (message.method === "window/logMessage") {
      this.output.appendLine(message.params.message);
    }
  }

  request(method, params) {
    const id = ++this.sequence;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      try {
        this.send({ jsonrpc: "2.0", id, method, params });
      } catch (error) {
        this.pending.delete(id);
        reject(error);
      }
    });
  }

  notify(method, params) {
    this.send({ jsonrpc: "2.0", method, params });
  }

  send(message) {
    if (!this.process?.stdin.writable) throw new Error("language server input is not writable");
    this.process.stdin.write(this.protocol.encodeMessage(message));
  }

  async stop() {
    if (this.stopPromise) return this.stopPromise;
    this.stopPromise = this.performStop();
    return this.stopPromise;
  }

  async performStop() {
    for (const disposable of this.disposables.splice(0)) disposable.dispose();
    if (!this.process || this.process.killed) return;

    try {
      await Promise.race([
        this.request("shutdown", null),
        new Promise((resolve) => setTimeout(resolve, 1000)),
      ]);
      if (this.process.stdin.writable) this.notify("exit", null);
    } finally {
      const processToStop = this.process;
      setTimeout(() => {
        if (!processToStop.killed) processToStop.kill();
      }, 1000).unref();
    }
  }

  dispose() {
    void this.stop();
  }
}

function resolveServer(extensionPath) {
  const configured = vscode.workspace.getConfiguration("orviscript").get("server.path", "").trim();
  if (configured) return configured;

  const executable = process.platform === "win32" ? "orvi-lsp.exe" : "orvi-lsp";
  const repositoryBuild = path.resolve(extensionPath, "..", "..", "target", "debug", executable);
  return fs.existsSync(repositoryBuild) ? repositoryBuild : "orvi-lsp";
}

module.exports = { activate, deactivate };
