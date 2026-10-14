import type { LogEvent } from '@ludion/protocol';
import { describe, expect, it } from 'vitest';
import { ProtocolError } from '../protocol/client';
import { diagnosticText, logEventToDiagnostic, toDiagnostic } from './diagnostic';

/** Builds a `LogEvent` for the helper under test. */
function log(level: LogEvent['level'], message: string): LogEvent {
  return { level, message };
}

describe('toDiagnostic', () => {
  it('keeps the raw message free of the code and the help, and carries related', () => {
    const err = new ProtocolError({
      code: 'L1014',
      message: 'm',
      help: 'h',
      related: [{ code: 'L1015', message: 'r' }],
    });
    expect(toDiagnostic(err)).toEqual({
      code: 'L1014',
      message: 'm',
      help: 'h',
      related: [{ code: 'L1015', message: 'r', help: null, related: [] }],
    });
  });

  it('yields no related entries for a wire error without any', () => {
    const diag = toDiagnostic(new ProtocolError({ code: 'L1014', message: 'm' }));
    expect(diag.related ?? []).toEqual([]);
    expect(diag.help).toBeNull();
  });
});

describe('diagnosticText', () => {
  it('renders the code, the message and the help as one line of text', () => {
    expect(diagnosticText({ code: 'L1234', message: 'bad pattern', help: 'fix the field' })).toBe(
      'L1234: bad pattern — fix the field',
    );
  });

  it('renders the message alone when there is no code or help', () => {
    expect(diagnosticText({ code: null, message: 'plain', help: null })).toBe('plain');
  });
});

describe('logEventToDiagnostic', () => {
  it('drops levels below the surfacing threshold', () => {
    expect(logEventToDiagnostic(log('trace', 'a'))).toBeNull();
    expect(logEventToDiagnostic(log('debug', 'b'))).toBeNull();
    expect(logEventToDiagnostic(log('info', 'scene loaded'))).toBeNull();
  });

  it('surfaces warnings and errors', () => {
    expect(logEventToDiagnostic(log('warn', 'heads up'))).not.toBeNull();
    expect(logEventToDiagnostic(log('error', 'boom'))).not.toBeNull();
  });

  it('prefers the structured diagnostic over the rendered message', () => {
    const event: LogEvent = {
      level: 'warn',
      message: 'error[L1144]: m\n  --> weapon/x/damage\n  help: h',
      diagnostic: {
        code: 'L1144',
        message: 'm',
        path: 'weapon/x/damage',
        help: 'h',
        related: [{ code: 'L1145', message: 'r' }],
      },
    };
    expect(logEventToDiagnostic(event)).toEqual({
      code: 'L1144',
      message: 'm',
      help: 'h',
      related: [{ code: 'L1145', message: 'r', help: null, related: [] }],
    });
  });

  it('falls back to the rustc-style head for a log line without the structured field', () => {
    const diag = logEventToDiagnostic(log('error', 'error[L1101]: texture not found: assets/checker.png'));
    expect(diag).toEqual({ code: 'L1101', message: 'texture not found: assets/checker.png', help: null });
  });

  it('extracts the help continuation line on the fallback path', () => {
    const message = 'error[L1101]: texture not found: assets/checker.png\n  help: place the PNG beside the scene file';
    expect(logEventToDiagnostic(log('error', message))).toEqual({
      code: 'L1101',
      message: 'texture not found: assets/checker.png',
      help: 'place the PNG beside the scene file',
    });
  });

  it('surfaces a non-diagnostic message verbatim with no code', () => {
    expect(logEventToDiagnostic(log('warn', 'something odd happened'))).toEqual({
      code: null,
      message: 'something odd happened',
      help: null,
    });
  });
});
