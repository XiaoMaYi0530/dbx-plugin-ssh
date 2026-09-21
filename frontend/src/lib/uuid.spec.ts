// uuid unit tests: crypto.randomUUID is only exposed in secure contexts
// (HTTPS / localhost), so the helper must feature-detect and fall back to a
// getRandomValues-based UUID v4 (RFC 4122). The fallback case reproduces the
// docker / plain-HTTP-on-LAN deployment where the plugin view died on startup.
import { afterEach, describe, expect, it, vi } from "vitest";

const realCrypto = globalThis.crypto;

const UUID_V4_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

afterEach(() => {
    vi.unstubAllGlobals();
});

describe("randomUUID", () => {
    it("returns a valid unique v4 UUID when crypto.randomUUID is available", async () => {
        const { randomUUID } = await import("./uuid");
        const first = randomUUID();
        const second = randomUUID();
        expect(first).toMatch(UUID_V4_PATTERN);
        expect(second).toMatch(UUID_V4_PATTERN);
        expect(second).not.toBe(first);
    });

    it("delegates to crypto.randomUUID untouched when the native API exists", async () => {
        vi.stubGlobal("crypto", { randomUUID: () => "native-id", getRandomValues: realCrypto.getRandomValues.bind(realCrypto) });
        const { randomUUID } = await import("./uuid");
        expect(randomUUID()).toBe("native-id");
    });

    it("falls back to getRandomValues-based v4 when crypto.randomUUID is undefined (plain HTTP)", async () => {
        // Insecure contexts keep crypto.getRandomValues but hide randomUUID.
        vi.stubGlobal("crypto", { getRandomValues: realCrypto.getRandomValues.bind(realCrypto) });
        const { randomUUID } = await import("./uuid");
        const first = randomUUID();
        const second = randomUUID();
        expect(first).toMatch(UUID_V4_PATTERN);
        expect(second).toMatch(UUID_V4_PATTERN);
        expect(second).not.toBe(first);
    });
});
