import type { ValidationResult } from "./types";

/** The last line of `text` with anything but whitespace on it, unchanged; null when there is none. */
export function lastNonEmptyLine(text: string): string | null {
  const lines = text.split(/\r?\n/);
  for (let i = lines.length - 1; i >= 0; i--) {
    if (lines[i].trim()) return lines[i];
  }
  return null;
}

/**
 * What a validation run said last: wad_simulator's last stdout line (its verdict), else its last
 * stderr line.
 */
export function validationLastLine(v: ValidationResult): string {
  return (
    lastNonEmptyLine(v.stdout) ?? lastNonEmptyLine(v.stderr) ?? "wad_simulator printed no output."
  );
}

/** The Apply & Play status for a patch that failed validation. */
export function validationFailedStatus(v: ValidationResult): string {
  return `Patch failed validation (exit ${v.exit_code ?? "?"}): ${validationLastLine(v)}`;
}

/** A step's status text with the latest line its tool printed, when there is one. */
export function withLatestLine(step: string, line: string | null): string {
  return line ? `${step} — ${line}` : step;
}
