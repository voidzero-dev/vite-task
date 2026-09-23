import { diagnose } from 'cbor2/diagnostic';

/** Keep byte strings readable without mistaking text containing h'...' for bytes. */
export function toEDN(bytes: Uint8Array): string {
  return diagnose(bytes).replace(
    /"(?:[^"\\]|\\.)*"|h'([0-9a-f]*)'/gi,
    (token, hex: string | undefined) => {
      if (hex === undefined) return token;
      const binary = Buffer.from(hex, 'hex');
      try {
        const text = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(binary);
        const escaped = JSON.stringify(text)
          .slice(1, -1)
          .replace(/\\.|'/g, (part) => {
            if (part === '\\"') return '"';
            return part === "'" ? "\\'" : part;
          });
        return `'${escaped}'`;
      } catch {
        return `b64'${binary.toString('base64')}'`;
      }
    },
  );
}
