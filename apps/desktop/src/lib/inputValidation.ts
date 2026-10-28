export function releaseReferenceProblem(value: string): string | null {
  if (/[\u0000-\u001f\u007f-\u009f]/.test(value)) return "The release reference cannot contain control characters.";
  if (Array.from(value.trim()).length > 256) return "Use at most 256 characters for the release reference.";
  return null;
}

export function noteProblem(value: string): string | null {
  if (/[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/.test(value)) {
    return "Notes may contain newlines and tabs, but no other control characters.";
  }
  if (Array.from(value.trim()).length > 4096) return "Use at most 4096 characters for the note.";
  return null;
}

export function parseTestCount(value: string): number | null {
  if (!value.trim()) return null;
  const count = Number(value);
  return Number.isInteger(count) && count >= 0 && count <= 4294967295 ? count : null;
}
