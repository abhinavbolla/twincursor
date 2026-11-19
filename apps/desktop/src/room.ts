// Room codes: 80 random bits from the OS, written as four groups of four characters from
// Crockford's base32 (digits and lowercase letters without i, l, o and u), such as
// "7kq2-m9xd-p4tz-hc3w". Anyone with the code and the app can join the call.

const ALPHABET = "0123456789abcdefghjkmnpqrstvwxyz";
const GROUPS = 4;
const GROUP_LENGTH = 4;
/** 16 characters of 5 bits each. */
export const CODE_LENGTH = GROUPS * GROUP_LENGTH;
export const CODE_BITS = CODE_LENGTH * 5;

const cryptoBytes = (n: number): Uint8Array => crypto.getRandomValues(new Uint8Array(n));

function group(chars: string): string {
  const parts: string[] = [];
  for (let i = 0; i < chars.length; i += GROUP_LENGTH) {
    parts.push(chars.slice(i, i + GROUP_LENGTH));
  }
  return parts.join("-");
}

/** A new code. 256 is a multiple of 32, so the low five bits of each byte are uniform. */
export function makeRoomCode(random: (n: number) => Uint8Array = cryptoBytes): string {
  const bytes = random(CODE_LENGTH);
  let chars = "";
  for (const byte of bytes) {
    chars += ALPHABET[byte & 31];
  }
  return group(chars);
}

/**
 * A pasted code in its usual form, or null if it isn't one. Case, spaces and dashes don't
 * matter, and letters often misread for digits are read as those digits (o as 0, i and l as 1).
 */
export function normalizeRoomCode(text: string): string | null {
  const chars = text
    .toLowerCase()
    .replace(/[\s-]/g, "")
    .replace(/o/g, "0")
    .replace(/[il]/g, "1");
  if (chars.length !== CODE_LENGTH || [...chars].some((c) => !ALPHABET.includes(c))) {
    return null;
  }
  return group(chars);
}
