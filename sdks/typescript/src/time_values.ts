function asDate(value: Date | string): Date {
  const date = value instanceof Date ? new Date(value.getTime()) : new Date(value);
  if (Number.isNaN(date.getTime())) throw new TypeError("invalid RFC3339 timestamp");
  return date;
}

function localRfc3339(date: Date): string {
  const offsetMinutes = -date.getTimezoneOffset();
  const sign = offsetMinutes >= 0 ? "+" : "-";
  const absolute = Math.abs(offsetMinutes);
  const hours = String(Math.floor(absolute / 60)).padStart(2, "0");
  const minutes = String(absolute % 60).padStart(2, "0");
  const shifted = new Date(date.getTime() + offsetMinutes * 60_000);
  return `${shifted.toISOString().replace("Z", "")}${sign}${hours}:${minutes}`;
}

/** Current Unix timestamp in seconds, returning zero only on invalid Date state. */
export function nowSecs(): number {
  return Math.floor(Date.now() / 1_000);
}

/** Current Unix timestamp in milliseconds. */
export function nowMillis(): number {
  return Date.now();
}

/** Current system-local timestamp with an explicit RFC3339 offset. */
export function nowLocal(): string {
  return localRfc3339(new Date());
}

/** Convert an instant to the system-local RFC3339 representation. */
export function toLocal(value: Date | string): string {
  return localRfc3339(asDate(value));
}

/** Serialize a UTC/offset instant using the system-local RFC3339 offset. */
export function localRfc3339Serialize(value: Date | string): string {
  return toLocal(value);
}

/** Parse a local or UTC RFC3339 value and return the same instant as Date. */
export function localRfc3339Deserialize(value: string): Date {
  return asDate(value);
}

export function optionLocalRfc3339Serialize(value: Date | string | null | undefined): string | null {
  return value == null ? null : toLocal(value);
}

export function optionLocalRfc3339Deserialize(value: string | null | undefined): Date | null {
  return value == null ? null : localRfc3339Deserialize(value);
}
