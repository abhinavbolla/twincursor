// Small pieces both surfaces draw: avatars, a line with names in it, keycaps, a closable line.

import { CloseIcon } from "./icons";
import { initials, type Phrase } from "./people";
import type { FaceView } from "./panelModel";

/** A round avatar: the photo, or initials on a neutral circle. */
export function Avatar({ face, size }: { face: FaceView; size: 24 | 28 | 36 }) {
  return face.avatar !== null ? (
    <img className={`avatar s${size}`} src={face.avatar} alt="" width={size} height={size} draggable={false} />
  ) : (
    <span className={`avatar s${size}`} aria-hidden="true">
      {initials(face.name)}
    </span>
  );
}

/** Up to three overlapping avatars and "+N" for the rest. */
export function Faces({ faces, extra }: { faces: FaceView[]; extra: number }) {
  if (faces.length === 0) {
    return null;
  }
  const label = faces.map((f) => f.name).join(", ") + (extra > 0 ? ` and ${extra} more` : "");
  return (
    <span className="faces" role="img" aria-label={label} title={label}>
      {faces.map((f, i) => (
        <Avatar key={`${f.name}-${i}`} face={f} size={24} />
      ))}
      {extra > 0 && <span className="faces-extra">+{extra}</span>}
    </span>
  );
}

/** A line of text; names in it are set apart and are what truncates. */
export function PhraseLine({ phrase, className }: { phrase: Phrase; className?: string }) {
  return (
    <span className={`phrase ${className ?? ""}`}>
      {phrase.map((part, i) =>
        typeof part === "string" ? (
          <span key={i} className="phrase-text">
            {part}
          </span>
        ) : (
          <bdi key={i} className="phrase-name">
            {part.name}
          </bdi>
        ),
      )}
    </span>
  );
}

/** A hotkey, one keycap per key. */
export function Keys({ keys, label }: { keys: string[]; label?: string }) {
  return (
    <span className="keys" role="img" aria-label={label ?? keys.join(" ")}>
      {keys.map((k) => (
        <kbd key={k}>{k}</kbd>
      ))}
    </span>
  );
}

/** One line of news, with a button to close it. */
export function NoticeRow({ text, tone, close }: { text: string; tone: "plain" | "bad"; close: () => void }) {
  return (
    <div className={`notice-row ${tone}`} role={tone === "bad" ? "alert" : "status"}>
      <span className="notice-text">{text}</span>
      <button type="button" className="icon-button tiny" aria-label="Close" title="Close" onClick={close}>
        <CloseIcon />
      </button>
    </div>
  );
}
