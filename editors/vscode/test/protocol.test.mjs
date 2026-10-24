import assert from "node:assert/strict";
import test from "node:test";

import { MessageReader, encodeMessage } from "../protocol.mjs";

test("encodes and decodes one LSP message", () => {
  const message = { jsonrpc: "2.0", id: 1, method: "initialize", params: { name: "żółw" } };
  const reader = new MessageReader();

  assert.deepEqual(reader.append(encodeMessage(message)), [message]);
});

test("waits for a complete body split across chunks", () => {
  const message = { jsonrpc: "2.0", method: "initialized", params: {} };
  const encoded = Buffer.from(encodeMessage(message));
  const reader = new MessageReader();

  assert.deepEqual(reader.append(encoded.subarray(0, 20)), []);
  assert.deepEqual(reader.append(encoded.subarray(20)), [message]);
});

test("reads consecutive messages and counts UTF-8 bytes", () => {
  const first = { jsonrpc: "2.0", method: "one", params: { value: "😀" } };
  const second = { jsonrpc: "2.0", method: "two", params: null };
  const reader = new MessageReader();

  assert.deepEqual(reader.append(encodeMessage(first) + encodeMessage(second)), [first, second]);
});
