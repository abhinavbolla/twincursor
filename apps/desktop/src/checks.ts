// What the helper reports (`tc-spike permissions`), and how each proof check is judged.

export type Access =
  | "granted"
  | "not_granted"
  | "not_asked"
  | "denied"
  | "restricted"
  | "not_applicable";

export interface PermissionStatus {
  screen_recording: Access;
  accessibility: Access;
  microphone: Access;
  pid: number;
  parent_pid: number;
  executable: string;
}

export interface WindowSample {
  id: number;
  title: string;
  opened_after_start: boolean;
  /** Share of the window's middle that shows the probe color, 0 to 1. */
  probe_color: number;
}

export interface CaptureReport {
  frame: [number, number] | null;
  error: string | null;
  included: WindowSample[];
  excluded: WindowSample[];
}

export interface PermissionsReport extends PermissionStatus {
  capture?: CaptureReport;
}

export interface StdinCloseResult {
  /** How long the helper took to exit after its stdin closed, or null if it didn't. */
  exited_after_ms: number | null;
}

export type Verdict = "pass" | "fail";

export interface CheckResult {
  id: string;
  label: string;
  verdict: Verdict;
  detail: string;
}

/** Title of the probe windows the app opens for the capture check (also in `main.rs`). */
export const PROBE_TITLE = "TwinCursor capture probe";
/** A probe seen in at least this share of its middle counts as visible in the capture. */
export const SEEN = 0.5;
/** A probe seen in at most this share counts as left out. */
export const LEFT_OUT = 0.02;
/** The helper must be gone this soon after the app's end of its stdin closes. */
export const EXIT_WITHIN_MS = 2000;

const ACCESS: readonly Access[] = [
  "granted",
  "not_granted",
  "not_asked",
  "denied",
  "restricted",
  "not_applicable",
];

type Json = Record<string, unknown>;

function isObject(value: unknown): value is Json {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function access(value: unknown): Access {
  const found = ACCESS.find((a) => a === value);
  if (found === undefined) {
    throw new Error(`unknown access value: ${String(value)}`);
  }
  return found;
}

function num(value: unknown, field: string): number {
  if (typeof value !== "number") {
    throw new Error(`${field} is not a number`);
  }
  return value;
}

function windows(value: unknown): WindowSample[] {
  if (!Array.isArray(value)) {
    throw new Error("expected a list of windows");
  }
  return value.map((w: unknown) => {
    if (!isObject(w)) {
      throw new Error("expected a window");
    }
    return {
      id: num(w.id, "id"),
      title: typeof w.title === "string" ? w.title : "",
      opened_after_start: w.opened_after_start === true,
      probe_color: num(w.probe_color, "probe_color"),
    };
  });
}

function capture(value: unknown): CaptureReport {
  if (!isObject(value)) {
    throw new Error("capture is not an object");
  }
  const frame = Array.isArray(value.frame)
    ? ([num(value.frame[0], "frame"), num(value.frame[1], "frame")] as [number, number])
    : null;
  return {
    frame,
    error: typeof value.error === "string" ? value.error : null,
    included: windows(value.included),
    excluded: windows(value.excluded),
  };
}

/** Reads the JSON object `tc-spike permissions` prints. Throws on anything else. */
export function parseReport(value: unknown): PermissionsReport {
  if (!isObject(value)) {
    throw new Error("the helper's report is not a JSON object");
  }
  const report: PermissionsReport = {
    screen_recording: access(value.screen_recording),
    accessibility: access(value.accessibility),
    microphone: access(value.microphone),
    pid: num(value.pid, "pid"),
    parent_pid: num(value.parent_pid, "parent_pid"),
    executable: typeof value.executable === "string" ? value.executable : "",
  };
  if (value.capture !== undefined) {
    report.capture = capture(value.capture);
  }
  return report;
}

const ACCESS_TEXT: Record<Access, string> = {
  granted: "allowed",
  not_granted: "not allowed yet",
  not_asked: "not asked yet",
  denied: "turned off in System Settings",
  restricted: "blocked by device management",
  not_applicable: "nothing to allow on this platform",
};

export function describeAccess(a: Access): string {
  return ACCESS_TEXT[a];
}

/** One result per permission, as the helper (not the app) sees it. */
export function permissionChecks(report: PermissionStatus): CheckResult[] {
  const rows: [string, string, Access, string][] = [
    [
      "screen",
      "Screen Recording reaches the helper",
      report.screen_recording,
      "Allow it in System Settings → Privacy & Security → Screen & System Audio Recording, then run the checks again (if it still fails, quit and reopen TwinCursor).",
    ],
    [
      "accessibility",
      "Accessibility reaches the helper",
      report.accessibility,
      "Turn TwinCursor on in System Settings → Privacy & Security → Accessibility.",
    ],
    ["mic", "Microphone reaches the helper", report.microphone, "Answer the microphone prompt."],
  ];
  return rows.map(([id, label, a, hint]) => ({
    id,
    label,
    verdict: a === "granted" ? "pass" : "fail",
    detail: a === "granted" ? describeAccess(a) : `${describeAccess(a)}. ${hint}`,
  }));
}

function probes(list: WindowSample[]): WindowSample[] {
  return list.filter((w) => w.title === PROBE_TITLE);
}

function percent(share: number): string {
  return `${Math.round(share * 100)}%`;
}

/** The capture checks: capture works, and the app's windows stay out of it. */
export function captureChecks(report: CaptureReport | undefined): CheckResult[] {
  const works: CheckResult = {
    id: "capture",
    label: "The helper can capture the screen",
    verdict: "fail",
    detail: "",
  };
  const leftOut: CheckResult = {
    id: "excluded",
    label: "The app's windows stay out of the capture",
    verdict: "fail",
    detail: "",
  };
  const late: CheckResult = {
    id: "excluded-late",
    label: "…including a window opened after capture started",
    verdict: "fail",
    detail: "",
  };
  if (report === undefined || report.error !== null || report.frame === null) {
    const why = report?.error ?? "no capture result";
    works.detail = why;
    leftOut.detail = late.detail = "needs a working capture first";
    return [works, leftOut, late];
  }
  works.verdict = "pass";
  works.detail = `frames of ${report.frame[0]}×${report.frame[1]}`;

  const seen = probes(report.included);
  const sensitive = seen.some((w) => w.probe_color >= SEEN);
  const early = probes(report.excluded).filter((w) => !w.opened_after_start);
  const later = probes(report.excluded).filter((w) => w.opened_after_start);
  const out = (list: WindowSample[]) => list.every((w) => w.probe_color <= LEFT_OUT);
  const shares = (list: WindowSample[]) => list.map((w) => percent(w.probe_color)).join(", ");

  if (!sensitive) {
    leftOut.detail = `the probe window wasn't visible even without leaving the app out (${shares(seen) || "no probe window"}), so this run can't tell; make sure nothing covers it`;
  } else if (early.length === 0) {
    leftOut.detail = "the probe window was gone during the second capture";
  } else {
    leftOut.verdict = out(early) ? "pass" : "fail";
    leftOut.detail = `probe visible ${shares(seen)} without the exclusion, ${shares(early)} with it`;
  }
  if (!sensitive) {
    late.detail = leftOut.detail;
  } else if (later.length === 0) {
    late.detail = "no window was opened during the capture";
  } else {
    late.verdict = out(later) ? "pass" : "fail";
    late.detail = `late probe visible ${shares(later)} with the exclusion`;
  }
  return [works, leftOut, late];
}

export function stdinCloseCheck(result: StdinCloseResult): CheckResult {
  const ms = result.exited_after_ms;
  const pass = ms !== null && ms <= EXIT_WITHIN_MS;
  return {
    id: "stdin",
    label: "The helper exits when the app goes away",
    verdict: pass ? "pass" : "fail",
    detail:
      ms === null
        ? "still running after its stdin closed"
        : `exited ${ms} ms after its stdin closed`,
  };
}

export function failed(id: string, label: string, error: unknown): CheckResult {
  return { id, label, verdict: "fail", detail: String(error) };
}
