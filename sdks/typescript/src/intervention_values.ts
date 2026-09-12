import type { ParamValue } from "./helpers.js";

/** Immutable intervention decision values; execution remains Host-owned. */
export type InterventionResult = Readonly<{
  block: boolean;
  blockReason?: string;
  injectedContext?: string;
  redirectTo?: string;
  cancel: boolean;
  modifiedArgs?: ParamValue;
}>;

export const InterventionResult = Object.freeze({
  allow(): InterventionResult {
    return Object.freeze({ block: false, cancel: false });
  },
  block(reason: string): InterventionResult {
    if (reason.trim().length === 0) throw new TypeError("block reason must not be empty");
    return Object.freeze({ block: true, blockReason: reason, cancel: false });
  },
  inject(context: string): InterventionResult {
    return Object.freeze({ block: false, injectedContext: context, cancel: false });
  },
  cancel(): InterventionResult {
    return Object.freeze({ block: false, cancel: true });
  },
  modifyArgs(args: ParamValue): InterventionResult {
    return Object.freeze({ block: false, cancel: false, modifiedArgs: args });
  },
});
