import { describe, expect, it } from 'vitest';
import { isConfinementRefusal } from './hostRefusal';

describe('isConfinementRefusal', () => {
  it('matches each host refusal', () => {
    expect(isConfinementRefusal('`x` is not an absolute path; the editor host resolves nothing')).toBe(true);
    expect(isConfinementRefusal('`../x` climbs with `..`; pass the resolved absolute path.')).toBe(true);
    expect(
      isConfinementRefusal('`/x` is outside the engine checkout and every Ludion project: only'),
    ).toBe(true);
    expect(isConfinementRefusal(new Error('`x` climbs with `..`; pass'))).toBe(true);
  });
  it('ignores ordinary errors and non-strings', () => {
    expect(isConfinementRefusal('No such file or directory (os error 2)')).toBe(false);
    expect(isConfinementRefusal(42)).toBe(false);
    expect(isConfinementRefusal(null)).toBe(false);
  });
});
