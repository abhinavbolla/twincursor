// Small choices the window remembers between launches (name, quality, voice). Never anything
// secret: the settings that matter live in the app's config file, written by the Rust side.

export function remembered(key: string, fallback: string): string {
  try {
    return window.localStorage.getItem(`twincursor.${key}`) ?? fallback;
  } catch {
    return fallback;
  }
}

export function remember(key: string, value: string): void {
  try {
    window.localStorage.setItem(`twincursor.${key}`, value);
  } catch {
    // Not remembered this time; nothing else depends on it.
  }
}
