export function encodeMessage(message) {
  const body = JSON.stringify(message);
  return `Content-Length: ${Buffer.byteLength(body, "utf8")}\r\n\r\n${body}`;
}

export class MessageReader {
  #buffer = Buffer.alloc(0);

  append(chunk) {
    this.#buffer = Buffer.concat([this.#buffer, Buffer.from(chunk)]);
    const messages = [];

    while (true) {
      const headerEnd = this.#buffer.indexOf("\r\n\r\n");
      if (headerEnd < 0) break;

      const header = this.#buffer.subarray(0, headerEnd).toString("ascii");
      const match = /^Content-Length:\s*(\d+)\s*$/im.exec(header);
      if (!match) throw new Error("LSP message is missing Content-Length");

      const length = Number.parseInt(match[1], 10);
      const bodyStart = headerEnd + 4;
      const bodyEnd = bodyStart + length;
      if (this.#buffer.length < bodyEnd) break;

      const body = this.#buffer.subarray(bodyStart, bodyEnd).toString("utf8");
      messages.push(JSON.parse(body));
      this.#buffer = this.#buffer.subarray(bodyEnd);
    }

    return messages;
  }
}
