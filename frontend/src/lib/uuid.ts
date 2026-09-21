// UUID v4 生成：crypto.randomUUID 仅在安全上下文（HTTPS / localhost）暴露，
// dbx 常以局域网 HTTP 访问（docker 部署），此时该 API 为 undefined，插件视图
// 一启动即抛 TypeError。getRandomValues 在非安全上下文仍可用，故做特性检测，
// 缺失时以随机字节手工拼出 RFC 4122 v4（同 ldap/frontend/src/lib/uuid.ts）。
export function randomUUID(): string {
  const c = globalThis.crypto;
  if (typeof c?.randomUUID === "function") {
    return c.randomUUID();
  }
  const bytes = c.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
  bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant 10xx
  const hex = [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}
