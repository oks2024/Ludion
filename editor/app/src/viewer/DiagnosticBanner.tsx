// Non-modal renderer for a command error: the L-code, the message, and the help line, styled as a
// quiet inline banner (the "errors are the UX budget" pillar — a rejected command explains itself to
// the human the same way it would to an agent, without a blocking dialog).

import type { DiagnosticView } from './diagnostic';

interface DiagnosticBannerProps {
  readonly diagnostic: DiagnosticView | null;
  readonly onDismiss?: () => void;
}

export function DiagnosticBanner({ diagnostic, onDismiss }: DiagnosticBannerProps) {
  if (diagnostic === null) {
    return null;
  }
  const related = diagnostic.related ?? [];
  return (
    <div className="diagnostic" role="alert">
      <div className="diagnostic-body">
        <span className="diagnostic-message">
          {diagnostic.code !== null && <code className="diagnostic-code">{diagnostic.code}</code>}
          {diagnostic.message}
        </span>
        {diagnostic.help !== null && <span className="diagnostic-help">{diagnostic.help}</span>}
        {related.length > 0 && (
          <details className="diagnostic-related">
            <summary>{related.length} related</summary>
            {related.map((r, i) => (
              <div className="diagnostic-related-row" key={`${r.code ?? ''}:${i}`}>
                <span className="diagnostic-message">
                  {r.code !== null && <code className="diagnostic-code">{r.code}</code>}
                  {r.message}
                </span>
                {r.help !== null && <span className="diagnostic-help">{r.help}</span>}
              </div>
            ))}
          </details>
        )}
      </div>
      {onDismiss !== undefined && (
        <button type="button" className="diagnostic-dismiss" onClick={onDismiss} aria-label="Dismiss error">
          ×
        </button>
      )}
    </div>
  );
}
