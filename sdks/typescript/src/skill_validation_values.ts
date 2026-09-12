/** Skill validation report values without running validation. */
export class SkillValidationReport {
  public constructor(
    public readonly path: string,
    public readonly violations: readonly string[] = [],
    public readonly warnings: readonly string[] = [],
  ) {
    Object.freeze(this.violations);
    Object.freeze(this.warnings);
    Object.freeze(this);
  }

  public isValid(): boolean {
    return this.violations.length === 0;
  }
}
