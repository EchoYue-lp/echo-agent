/** Agent turn stream modes projected as local immutable values. */
export const TurnMode = Object.freeze({
  Chat: "chat",
  Execute: "execute",
} as const);

export type TurnMode = (typeof TurnMode)[keyof typeof TurnMode];

export function turnModeAsStr(mode: TurnMode): string {
  if (mode !== TurnMode.Chat && mode !== TurnMode.Execute) throw new TypeError("invalid turn mode");
  return mode;
}
