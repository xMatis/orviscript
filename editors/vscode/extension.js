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

    const initialization = await this.request("initialize", {
      processId: process.pid,
      clientInfo: { name: "OrviScript VS Code", version: "0.5.0" },
      rootUri: vscode.workspace.workspaceFolders?.[0]?.uri.toString() ?? null,
      capabilities: {
        general: { positionEncodings: ["utf-16"] },
        textDocument: {
          completion: { completionItem: {} },
          definition: {},
          hover: { contentFormat: ["markdown", "plaintext"] },
          publishDiagnostics: { versionSupport: true },
          references: {},
          rename: { prepareSupport: true },
          semanticTokens: {
            requests: { full: true },
            tokenTypes: ["variable", "function", "parameter", "keyword", "string", "number", "comment", "operator", "type", "property"],
            tokenModifiers: ["declaration", "readonly", "defaultLibrary"],
            formats: ["relative"],
          },
        },
      },
    });
    this.notify("initialized", {});
    this.ready = true;
    this.registerLanguageFeatures(initialization.capabilities);

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

  registerLanguageFeatures(capabilities) {
    const selector = { language: "orviscript" };

    if (capabilities.completionProvider) {
      this.disposables.push(vscode.languages.registerCompletionItemProvider(selector, {
        provideCompletionItems: async (document, position) => {
          const response = await this.request("textDocument/completion", documentPosition(document, position));
          const items = Array.isArray(response) ? response : response?.items ?? [];
          return items.map((item) => {
            const completion = new vscode.CompletionItem(item.label, completionKind(item.kind));
            completion.detail = item.detail;
            completion.documentation = item.documentation?.value ?? item.documentation;
            completion.insertText = item.insertText;
            return completion;
          });
        },
      }));
    }

    if (capabilities.hoverProvider) {
      this.disposables.push(vscode.languages.registerHoverProvider(selector, {
        provideHover: async (document, position) => {
          const response = await this.request("textDocument/hover", documentPosition(document, position));
          if (!response) return undefined;
          const value = response.contents?.value ?? response.contents;
          const range = response.range && new vscode.Range(
            response.range.start.line,
            response.range.start.character,
            response.range.end.line,
            response.range.end.character,
          );
          return new vscode.Hover(new vscode.MarkdownString(value), range);
        },
      }));
    }

    if (capabilities.definitionProvider) {
      this.disposables.push(vscode.languages.registerDefinitionProvider(selector, {
        provideDefinition: async (document, position) => {
          const response = await this.request("textDocument/definition", documentPosition(document, position));
          if (!response) return undefined;
          return (Array.isArray(response) ? response : [response]).map(asLocation);
        },
      }));
    }

    if (capabilities.referencesProvider) {
      this.disposables.push(vscode.languages.registerReferenceProvider(selector, {
        provideReferences: async (document, position, context) => {
          const response = await this.request("textDocument/references", {
            ...documentPosition(document, position),
            context: { includeDeclaration: context.includeDeclaration },
          });
          return response?.map(asLocation) ?? [];
        },
      }));
    }

    if (capabilities.renameProvider) {
      this.disposables.push(vscode.languages.registerRenameProvider(selector, {
        prepareRename: async (document, position) => {
          const response = await this.request("textDocument/prepareRename", documentPosition(document, position));
          if (!response) throw new Error("The selected symbol cannot be renamed");
          const range = asRange(response.range ?? response);
          return response.placeholder ? { range, placeholder: response.placeholder } : range;
        },
        provideRenameEdits: async (document, position, newName) => {
          const response = await this.request("textDocument/rename", {
            ...documentPosition(document, position),
            newName,
          });
          if (!response) throw new Error("The selected symbol cannot be renamed");
          const edit = new vscode.WorkspaceEdit();
          for (const [uri, changes] of Object.entries(response?.changes ?? {})) {
            for (const change of changes) {
              edit.replace(vscode.Uri.parse(uri), asRange(change.range), change.newText);
            }
          }
          return edit;
        },
      }));
    }

    if (capabilities.documentFormattingProvider) {
      this.disposables.push(vscode.languages.registerDocumentFormattingEditProvider(selector, {
        provideDocumentFormattingEdits: async (document, options) => {
          const response = await this.request("textDocument/formatting", {
            textDocument: { uri: document.uri.toString() },
            options: {
              tabSize: options.tabSize,
              insertSpaces: options.insertSpaces,
            },
          });
          return response?.map((edit) => vscode.TextEdit.replace(asRange(edit.range), edit.newText)) ?? [];
        },
      }));
    }

    const semantic = capabilities.semanticTokensProvider;
    if (semantic?.legend) {
      const legend = new vscode.SemanticTokensLegend(
        semantic.legend.tokenTypes,
        semantic.legend.tokenModifiers,
      );
      this.disposables.push(vscode.languages.registerDocumentSemanticTokensProvider(selector, {
        provideDocumentSemanticTokens: async (document) => {
          const response = await this.request("textDocument/semanticTokens/full", {
            textDocument: { uri: document.uri.toString() },
          });
          return new vscode.SemanticTokens(Uint32Array.from(response?.data ?? []), response?.resultId);
        },
      }, legend));
    }
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

function documentPosition(document, position) {
  return {
    textDocument: { uri: document.uri.toString() },
    position: { line: position.line, character: position.character },
  };
}

function completionKind(kind) {
  const kinds = {
    3: vscode.CompletionItemKind.Function,
    4: vscode.CompletionItemKind.Constructor,
    6: vscode.CompletionItemKind.Variable,
    12: vscode.CompletionItemKind.Value,
    13: vscode.CompletionItemKind.Enum,
    14: vscode.CompletionItemKind.Keyword,
    20: vscode.CompletionItemKind.EnumMember,
    21: vscode.CompletionItemKind.Constant,
    22: vscode.CompletionItemKind.Struct,
    25: vscode.CompletionItemKind.TypeParameter,
  };
  return kinds[kind] ?? vscode.CompletionItemKind.Text;
}

function asRange(range) {
  return new vscode.Range(
    range.start.line,
    range.start.character,
    range.end.line,
    range.end.character,
  );
}

function asLocation(location) {
  return new vscode.Location(vscode.Uri.parse(location.uri), asRange(location.range));
}

module.exports = { activate, deactivate };
