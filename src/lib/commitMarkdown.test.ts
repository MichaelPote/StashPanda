import { describe, expect, it } from "vitest";
import { renderDescription, splitTrailers } from "./commitMarkdown";

describe("splitTrailers", () => {
  it("splits the trailer block off the end", () => {
    expect(splitTrailers("Some text.\n\nSigned-off-by: A <a@x.org>\nCo-Authored-By: B <b@x.org>")).toEqual({
      text: "Some text.",
      trailers: ["Signed-off-by: A <a@x.org>", "Co-Authored-By: B <b@x.org>"],
    });
  });

  it("joins a continuation line onto its trailer", () => {
    expect(splitTrailers("Text.\n\nFixes: something long\n  that wraps").trailers).toEqual(["Fixes: something long that wraps"]);
  });

  it("leaves a last paragraph alone when any line is not a trailer", () => {
    expect(splitTrailers("Text.\n\nNote: this\nis prose.")).toEqual({ text: "Text.\n\nNote: this\nis prose.", trailers: [] });
  });

  it("handles a description that is only trailers, with CRLF endings", () => {
    expect(splitTrailers("Signed-off-by: A <a@x.org>\r\n")).toEqual({ text: "", trailers: ["Signed-off-by: A <a@x.org>"] });
  });
});

describe("renderDescription", () => {
  it("joins hard-wrapped lines and renders lists, code and links", () => {
    const html = renderDescription("First line\nwrapped.\n\n- one\n- `two`\n\nSee https://example.com");
    expect(html).toContain("<p>First line\nwrapped.</p>");
    expect(html).toContain("<li><code>two</code></li>");
    expect(html).toContain('<a href="https://example.com">https://example.com</a>');
  });

  it("shows raw HTML as text", () => {
    expect(renderDescription('<img src=x onerror="alert(1)">')).toContain("&lt;img src=x onerror=&quot;alert(1)&quot;&gt;");
  });

  it("does not load images or follow javascript: links", () => {
    const html = renderDescription("![pixel](https://tracker.example/p.png) [x](javascript:alert(1))");
    expect(html).not.toContain("<img");
    expect(html).not.toContain('href="javascript:');
  });

  it("draws trailers one per line, escaped", () => {
    expect(renderDescription("Body.\n\nSigned-off-by: A <a@x.org>\nCo-Authored-By: B <b@x.org>")).toBe(
      '<p>Body.</p>\n<p class="md-trailers">Signed-off-by: A &lt;a@x.org&gt;<br>Co-Authored-By: B &lt;b@x.org&gt;</p>\n',
    );
  });
});
