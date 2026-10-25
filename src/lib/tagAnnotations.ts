import { commands, type TagAnnotation } from "@/lib/bindings";
import { formatAbsoluteTime, formatRelativeTime } from "@/lib/format";
import { unwrap } from "@/lib/ipc";

/**
 * Tag annotations for the tooltips on tag pills, loaded the first time a pill is hovered and kept
 * by repository, tag and commit, so hovering again asks Git nothing. A tag moved to another commit
 * is a new entry. A failed load is forgotten, so the next hover tries again.
 */
const loaded = new Map<string, Promise<TagAnnotation | null>>();

export function loadTagAnnotation(key: string, name: string, target: string): Promise<TagAnnotation | null> {
  const id = `${key}\0${name}\0${target}`;
  let annotation = loaded.get(id);
  if (!annotation) {
    annotation = unwrap(commands.getTagAnnotation(key, name)).catch(() => {
      loaded.delete(id);
      return null;
    });
    loaded.set(id, annotation);
  }
  return annotation;
}

/**
 * A tag pill's tooltip: the tag's name as the heading, then its whole message and who tagged it
 * when. A lightweight tag says it has none of those; one not loaded yet, or gone, shows its ref name.
 */
export function tagTooltip(fullName: string, short: string, annotation: TagAnnotation | null | undefined, nowMs: number = Date.now()): { heading: string; content: string } {
  if (!annotation) return { heading: short, content: fullName };
  if (!annotation.annotated) return { heading: short, content: "A lightweight tag: it has no message, tagger or date of its own." };
  const who = annotation.taggerEmail ? `${annotation.taggerName} <${annotation.taggerEmail}>` : annotation.taggerName;
  const when = annotation.taggerTime > 0 ? `, ${formatRelativeTime(annotation.taggerTime, nowMs)} (${formatAbsoluteTime(annotation.taggerTime)})` : "";
  const message = annotation.message || "(no message)";
  return { heading: short, content: `${message}\n\nTagged by ${who}${when}` };
}
