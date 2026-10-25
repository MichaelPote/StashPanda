/**
 * Commit descriptions as Markdown, for the Files panel's expanded message
 * (`appearance.markdownDescriptions`). The subject line is never rendered this way.
 *
 * A description comes from whoever wrote the commit, so nothing in it is trusted: raw HTML is
 * shown as text, images are not loaded (a remote image would tell its server who looked at the
 * commit), and the result goes through DOMPurify as well.
 */
import DOMPurify from "dompurify";
import MarkdownIt from "markdown-it";

const md = new MarkdownIt({ html: false, linkify: true, typographer: false }).disable("image");

/** One line of a Git trailer block, such as `Signed-off-by: Name <email>`. */
const TRAILER_LINE = /^[A-Za-z0-9][A-Za-z0-9-]*: \S/;

/**
 * Splits the trailer block off the end of a description. Git counts the last paragraph as
 * trailers when each of its lines is `Token: value`, or a continuation of the line above that
 * starts with whitespace. Markdown would join those lines into one, so they are drawn apart.
 */
export function splitTrailers(body: string): { text: string; trailers: string[] } {
  const normalised = body.replace(/\r\n/g, "\n").trimEnd();
  const split = normalised.lastIndexOf("\n\n");
  const last = split < 0 ? normalised : normalised.slice(split + 2);
  const lines = last.split("\n");
  const isTrailers = lines.length > 0 && TRAILER_LINE.test(lines[0] as string) && lines.every((l) => TRAILER_LINE.test(l) || /^\s+\S/.test(l));
  if (!isTrailers) return { text: normalised, trailers: [] };
  const trailers: string[] = [];
  for (const line of lines) {
    if (TRAILER_LINE.test(line)) trailers.push(line);
    else trailers[trailers.length - 1] += ` ${line.trim()}`;
  }
  return { text: split < 0 ? "" : normalised.slice(0, split), trailers };
}

/** The description as HTML, before sanitising. Exported for the tests, which have no DOM. */
export function renderDescription(body: string): string {
  const { text, trailers } = splitTrailers(body);
  let html = text.length > 0 ? md.render(text) : "";
  if (trailers.length > 0) {
    html += `<p class="md-trailers">${trailers.map((t) => md.utils.escapeHtml(t)).join("<br>")}</p>\n`;
  }
  return html;
}

/** The description as HTML that is safe to put in the page with `v-html`. */
export function descriptionHtml(body: string): string {
  return DOMPurify.sanitize(renderDescription(body));
}

/** The link a click in a rendered description should open, if it is one StashPanda will open. */
export function descriptionLink(target: EventTarget | null): string | null {
  const href = target instanceof Element ? target.closest("a")?.getAttribute("href") : null;
  return href && /^(https?:|mailto:)/i.test(href) ? href : null;
}
