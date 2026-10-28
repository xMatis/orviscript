function inlineDiagnosticEntries(diagnostics, enabled) {
  if (!enabled) return [];
  const seenLines = new Set();
  return diagnostics.flatMap((diagnostic) => {
    if (!String(diagnostic.code ?? "").startsWith("ORVI")) return [];
    const line = diagnostic.range.end.line;
    if (seenLines.has(line)) return [];
    seenLines.add(line);
    return [{
      line,
      text: `${diagnostic.code}: ${diagnostic.message}`,
    }];
  });
}

module.exports = { inlineDiagnosticEntries };
