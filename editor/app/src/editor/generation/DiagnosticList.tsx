// A verb's last word: its diagnostics as `L####` rows, or its stderr when it printed no object.

import type { VerbMessages } from './model';

export function DiagnosticList({ messages }: { readonly messages: VerbMessages }) {
  return (
    <>
      {messages.diagnostics.length > 0 && (
        <div className="vx-diags">
          {messages.diagnostics.map((diag, index) => (
            <div key={`${diag.code}-${index}`} className={`vx-diag ${diag.severity}`}>
              <span className="vx-diag-code">{diag.code}</span>
              <span className="vx-diag-msg">{diag.message}</span>
              {diag.path !== null && <span className="vx-diag-loc">{diag.path}</span>}
              {diag.help !== null && <span className="vx-diag-help">{diag.help}</span>}
            </div>
          ))}
        </div>
      )}
      {messages.stderr !== null && <pre className="gen-stderr">{messages.stderr}</pre>}
    </>
  );
}
