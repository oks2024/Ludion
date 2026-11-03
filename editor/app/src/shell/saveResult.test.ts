import { describe, expect, it } from 'vitest';
import type { ResponseBody } from '@ludion/protocol';
import { isSaveConflict, saveFindings, SCENE_SAVE_CONFLICT_CODE } from './saveResult';

const saved = (diagnostics?: unknown[]): ResponseBody =>
  ({ kind: 'scene_saved', path: 'a.yaml', ...(diagnostics ? { diagnostics } : {}) }) as unknown as ResponseBody;

describe('isSaveConflict', () => {
  it('matches only the conflict code', () => {
    expect(isSaveConflict({ code: SCENE_SAVE_CONFLICT_CODE, message: 'm', help: null })).toBe(true);
    expect(isSaveConflict({ code: 'L3059', message: 'm', help: null })).toBe(false);
    expect(isSaveConflict({ code: null, message: 'm', help: null })).toBe(false);
  });
});

describe('saveFindings', () => {
  it('is null for a clean save, with or without the field', () => {
    expect(saveFindings(saved())).toBeNull();
    expect(saveFindings(saved([]))).toBeNull();
  });

  it('lifts a single finding unchanged', () => {
    expect(saveFindings(saved([{ code: 'L1', message: 'w', severity: 'warning' }]))).toMatchObject({
      code: 'L1',
      message: 'w',
    });
  });

  it('prefers an error over a warning and counts several', () => {
    const view = saveFindings(
      saved([
        { code: 'L1', message: 'warn', severity: 'warning' },
        { code: 'L2', message: 'bad' },
      ]),
    );
    expect(view).toMatchObject({ code: 'L2' });
    expect(view?.message).toContain('bad');
    expect(view?.message).toContain('2');
  });
});
