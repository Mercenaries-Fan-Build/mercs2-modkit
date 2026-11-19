import { describe, expect, it } from "vitest";
import {
  lastNonEmptyLine,
  validationFailedStatus,
  validationLastLine,
  withLatestLine,
} from "./statusText";
import type { ValidationResult } from "./types";

const VERDICT =
  "VERDICT: FAILED — 1 block never installed (blocks\\VZ\\resident_P000_Q3.block: 7022 entries, ring 7018)";

function result(stdout: string, stderr: string, exit_code: number | null = 1): ValidationResult {
  return { ok: false, exit_code, stdout, stderr };
}

describe("lastNonEmptyLine", () => {
  it("skips trailing blank and whitespace-only lines", () => {
    expect(lastNonEmptyLine(`report\n${VERDICT}\n\n   \n\t\n`)).toBe(VERDICT);
  });

  it("splits CRLF output and keeps the line otherwise verbatim", () => {
    expect(lastNonEmptyLine("first\r\n  indented verdict  \r\n\r\n")).toBe("  indented verdict  ");
  });

  it("is null for empty or whitespace-only text", () => {
    expect(lastNonEmptyLine("")).toBeNull();
    expect(lastNonEmptyLine(" \n\r\n\t")).toBeNull();
  });
});

describe("validation failure text", () => {
  it("shows the last stdout line, ignoring stderr", () => {
    const v = result(`block report\n${VERDICT}\n`, "warning on stderr\n");
    expect(validationLastLine(v)).toBe(VERDICT);
    expect(validationFailedStatus(v)).toBe(`Patch failed validation (exit 1): ${VERDICT}`);
  });

  it("shows the last stderr line when stdout is empty", () => {
    const v = result(" \n", "error: cannot open WAD\n\n", 2);
    expect(validationFailedStatus(v)).toBe("Patch failed validation (exit 2): error: cannot open WAD");
  });

  it("says plainly that there was no output when both are empty", () => {
    const v = result("", "\n", null);
    expect(validationFailedStatus(v)).toBe(
      "Patch failed validation (exit ?): wad_simulator printed no output.",
    );
  });
});

describe("withLatestLine", () => {
  it("puts the latest line after the step", () => {
    expect(withLatestLine("Building…", "fetch_package biua_ps3: downloaded 120 of 389 MB")).toBe(
      "Building… — fetch_package biua_ps3: downloaded 120 of 389 MB",
    );
  });

  it("is the step alone when nothing has been printed", () => {
    expect(withLatestLine("Building…", null)).toBe("Building…");
  });
});
