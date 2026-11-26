// Names as the panel and the call bar set them: initials for an avatar without a photo, the
// short name the bar uses, and lists of names in a sentence.

const segmenter =
  typeof Intl !== "undefined" && "Segmenter" in Intl ? new Intl.Segmenter(undefined, { granularity: "grapheme" }) : null;

/** The characters a person sees, so an accented letter or a joined emoji stays whole. */
function graphemes(text: string): string[] {
  return segmenter === null ? Array.from(text) : Array.from(segmenter.segment(text), (s) => s.segment);
}

const words = (name: string): string[] => name.trim().split(/\s+/u).filter((w) => w !== "");

/** A letter from a script with upper and lower case (Latin, Greek, Cyrillic, …). */
const cased = (ch: string): boolean => /^[\p{Lu}\p{Ll}\p{Lt}]/u.test(ch);

/**
 * Up to two initials for an avatar: "Sam Rivera" → "SR". Names in a script without case
 * (Chinese, Japanese, Arabic, …) get their first character only, which is how they are usually
 * abbreviated.
 */
export function initials(name: string): string {
  const parts = words(name);
  const first = graphemes(parts[0] ?? "")[0];
  if (first === undefined) {
    return "?";
  }
  if (!cased(first)) {
    return first;
  }
  const last = parts.length > 1 ? graphemes(parts[parts.length - 1] ?? "")[0] : undefined;
  return (first + (last !== undefined && cased(last) ? last : "")).toLocaleUpperCase();
}

/** The first word of a name, for the bar's one line: "Sam Rivera" → "Sam". */
export function shortName(name: string): string {
  return words(name)[0] ?? name;
}

/** "Sam", "Sam and Kim", "Sam and 2 others". */
export function joinNames(names: readonly string[]): string {
  const [first, second] = names;
  if (first === undefined) {
    return "";
  }
  if (second === undefined) {
    return first;
  }
  return names.length === 2 ? `${first} and ${second}` : `${first} and ${names.length - 1} others`;
}

/**
 * A line of text with names in it. Names are set apart (weight 500, isolated for right-to-left
 * scripts, and the part that truncates); everything else is plain text.
 */
export type Phrase = ReadonlyArray<string | { name: string }>;

export const phraseText = (phrase: Phrase): string =>
  phrase.map((part) => (typeof part === "string" ? part : part.name)).join("");
