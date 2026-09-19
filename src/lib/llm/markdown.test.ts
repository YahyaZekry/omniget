import { describe, expect, it } from "vitest";
import { escapeHtml, isSafeUrl, renderSafeMarkdownSync, sanitizeHtml } from "./markdown";

describe("isSafeUrl", () => {
  it("accepts http, https, mailto and relative links", () => {
    expect(isSafeUrl("https://example.com")).toBe(true);
    expect(isSafeUrl("http://example.com")).toBe(true);
    expect(isSafeUrl("mailto:a@b.c")).toBe(true);
    expect(isSafeUrl("/downloads")).toBe(true);
    expect(isSafeUrl("./x.png")).toBe(true);
  });

  it("rejects script-bearing and protocol-relative urls", () => {
    expect(isSafeUrl("javascript:alert(1)")).toBe(false);
    expect(isSafeUrl("  JavaScript:alert(1)")).toBe(false);
    expect(isSafeUrl("data:text/html;base64,PHNjcmlwdD4=")).toBe(false);
    expect(isSafeUrl("vbscript:msgbox")).toBe(false);
    expect(isSafeUrl("//evil.tld")).toBe(false);
    expect(isSafeUrl("")).toBe(false);
  });
});

describe("sanitizeHtml without a DOM", () => {
  it("escapes everything instead of letting raw html through", () => {
    // vitest runs in the node environment: no `document`, so the fallback wins.
    expect(sanitizeHtml("<script>alert(1)</script>")).toBe(
      "&lt;script&gt;alert(1)&lt;/script&gt;",
    );
  });
});

describe("escapeHtml", () => {
  it("covers the four characters that break out of text", () => {
    expect(escapeHtml(`<a href="x">&</a>`)).toBe("&lt;a href=&quot;x&quot;&gt;&amp;&lt;/a&gt;");
  });
});

describe("renderSafeMarkdownSync", () => {
  it("returns escaped text on the first pass and caches the rendered html", async () => {
    const cache = new Map<string, string>();
    const first = renderSafeMarkdownSync("<b>hi</b>\nthere", cache);
    expect(first).toContain("&lt;b&gt;hi&lt;/b&gt;");
    expect(first).toContain("<br>");
    expect(renderSafeMarkdownSync("", cache)).toBe("");
  });
});
