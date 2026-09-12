/** Prompt section diagnostics projected without owning prompt compilation. */
export interface PromptSectionDiagnostic {
  readonly id: string;
  readonly source: string;
}

export class PromptDiagnostics {
  public readonly sections: PromptSectionDiagnostic[];

  public constructor(sections: readonly PromptSectionDiagnostic[] = []) {
    this.sections = sections.map((section) => Object.freeze({ ...section }));
  }

  public record(id: string, source: string): void {
    this.sections.push(Object.freeze({ id, source }));
  }

  public count(id: string): number {
    return this.sections.filter((section) => section.id === id).length;
  }
}
