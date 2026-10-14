// CodeMirror 6 host for the Game Designer view's code panel. Imperative CM lives here, behind a thin
// React shell keyed by file path (a new file remounts with a fresh doc). The parent (CodePanel) owns
// the save/watch state machine and the dev-status diagnostics; this component only: renders the doc,
// reports user edits (never programmatic reloads), exposes Ctrl+S save + Ctrl+F search, highlights &
// scrolls to a target fn line, and paints dev diagnostics via the lint gutter. Themed to the Meridian
// tokens from index.css. No LSP in v1 (open-in-IDE is the escape hatch; the drawer's Claude is the
// smart layer).

import { useEffect, useImperativeHandle, useRef, type Ref } from 'react';
import {
  Decoration,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
  type DecorationSet,
} from '@codemirror/view';
import { Annotation, EditorState, StateEffect, StateField } from '@codemirror/state';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { highlightSelectionMatches, search, searchKeymap } from '@codemirror/search';
import { bracketMatching, HighlightStyle, indentOnInput, syntaxHighlighting } from '@codemirror/language';
import { lintGutter, setDiagnostics, type Diagnostic as CMDiagnostic } from '@codemirror/lint';
import { rust } from '@codemirror/lang-rust';
import { tags as t } from '@lezer/highlight';

/** A dev-status diagnostic reduced to what the editor needs to place a decoration. */
export interface EditorDiag {
  readonly line: number;
  readonly column: number | null;
  readonly severity: 'error' | 'warning';
  readonly message: string;
}

/** Imperative handle the parent uses to reload disk content and read the current doc back. */
export interface RustEditorHandle {
  /** Replace the whole doc from disk WITHOUT counting as a user edit (a reload). */
  replaceDoc(text: string): void;
  /** The live editor content (for a save). */
  currentDoc(): string;
}

interface RustEditorProps {
  /** The file's content at mount (the component is keyed by path, so this is the fresh doc). */
  readonly initialDoc: string;
  /** 1-based fn line to highlight + scroll to (0 = none). */
  readonly targetLine: number;
  /** Diagnostics for THIS file (already path-matched by the parent). */
  readonly diagnostics: readonly EditorDiag[];
  readonly onChange: (doc: string) => void;
  readonly onSave: () => void;
  readonly handleRef: Ref<RustEditorHandle>;
}

// A programmatic reload marks its transaction so the change listener doesn't treat it as a user edit.
const External = Annotation.define<boolean>();

// The target-fn-line highlight, driven by a StateEffect so it survives edits (mapped through changes).
const setTargetLine = StateEffect.define<number>();
const targetLineField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(deco, tr) {
    let next = deco.map(tr.changes);
    for (const e of tr.effects) {
      if (e.is(setTargetLine)) {
        const ln = e.value;
        if (ln >= 1 && ln <= tr.state.doc.lines) {
          next = Decoration.set([Decoration.line({ class: 'cm-target-line' }).range(tr.state.doc.line(ln).from)]);
        } else {
          next = Decoration.none;
        }
      }
    }
    return next;
  },
  provide: (f) => EditorView.decorations.from(f),
});

// Meridian-toned syntax colors (index.css has no syntax tokens; these are tuned to its palette).
const meridianHighlight = HighlightStyle.define([
  { tag: [t.keyword, t.definitionKeyword, t.modifier, t.moduleKeyword], color: '#c792ff' },
  { tag: [t.name, t.deleted, t.character, t.macroName], color: '#edeef2' },
  { tag: [t.function(t.variableName), t.labelName], color: '#82aaff' },
  { tag: [t.propertyName, t.function(t.propertyName)], color: '#82aaff' },
  { tag: [t.typeName, t.namespace, t.className], color: '#7fdbca' },
  { tag: [t.string, t.inserted, t.special(t.string)], color: '#c3e88d' },
  { tag: [t.number, t.bool, t.null, t.atom], color: '#f78c6c' },
  { tag: [t.comment, t.lineComment, t.blockComment], color: '#5c6270', fontStyle: 'italic' },
  { tag: [t.operator, t.punctuation, t.separator, t.bracket], color: '#9ba0ab' },
  { tag: [t.attributeName, t.meta, t.annotation], color: '#d9a344' },
  { tag: [t.self, t.constant(t.variableName)], color: '#f78c6c' },
  { tag: [t.invalid], color: '#e5604c' },
]);

const meridianTheme = EditorView.theme(
  {
    '&': { color: 'var(--text)', backgroundColor: '#0b0c0f', height: '100%', fontSize: '12px' },
    '&.cm-focused': { outline: 'none' },
    '.cm-scroller': { fontFamily: 'var(--font-mono)', lineHeight: '1.55' },
    '.cm-content': { caretColor: 'var(--accent)' },
    '.cm-gutters': { backgroundColor: '#0b0c0f', color: 'var(--text-3)', border: 'none' },
    '.cm-activeLine': { backgroundColor: 'rgba(255,255,255,0.035)' },
    '.cm-activeLineGutter': { backgroundColor: 'rgba(255,255,255,0.035)', color: 'var(--text-2)' },
    '.cm-target-line': { backgroundColor: 'var(--accent-dim)', boxShadow: 'inset 2px 0 0 var(--accent)' },
    '.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--accent)' },
    '.cm-selectionBackground, .cm-content ::selection': { backgroundColor: 'var(--accent-dim2)' },
    '&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground': { backgroundColor: 'var(--accent-dim2)' },
    '.cm-searchMatch': { backgroundColor: 'var(--accent-dim2)', outline: '1px solid var(--accent-dim2)' },
    '.cm-searchMatch.cm-searchMatch-selected': { backgroundColor: 'var(--accent-dim)' },
    '.cm-selectionMatch': { backgroundColor: 'rgba(255,255,255,0.06)' },
    '.cm-panels': { backgroundColor: 'var(--raised)', color: 'var(--text)', borderColor: 'var(--border)' },
    '.cm-panels.cm-panels-top': { borderBottom: '1px solid var(--border)' },
    '.cm-panel input, .cm-panel button': { fontFamily: 'var(--font-ui)' },
    '.cm-textfield': { backgroundColor: 'var(--base)', color: 'var(--text)', border: '1px solid var(--border)' },
    '.cm-button': { backgroundColor: 'var(--raised)', color: 'var(--text)', border: '1px solid var(--border)', backgroundImage: 'none' },
    '.cm-tooltip': { backgroundColor: 'var(--overlay)', color: 'var(--text)', border: '1px solid var(--border)' },
    '.cm-tooltip.cm-tooltip-lint': { maxWidth: '360px' },
    '.cm-lintRange-error': { textDecorationColor: 'var(--danger)' },
    '.cm-lintRange-warning': { textDecorationColor: 'var(--warn)' },
  },
  { dark: true },
);

/** Convert path-matched dev diagnostics to CodeMirror lint diagnostics against the live doc: the
 *  span underlines from the column to end of line (rustc gives us line/col, not a full span). */
function toCMDiagnostics(view: EditorView, diags: readonly EditorDiag[]): CMDiagnostic[] {
  const doc = view.state.doc;
  const out: CMDiagnostic[] = [];
  for (const d of diags) {
    if (d.line < 1 || d.line > doc.lines) {
      continue;
    }
    const line = doc.line(d.line);
    const col = d.column !== null && d.column >= 1 ? Math.min(d.column - 1, line.length) : 0;
    const from = line.from + col;
    const to = line.to > from ? line.to : from;
    out.push({ from, to, severity: d.severity, message: d.message });
  }
  return out;
}

export function RustEditor({ initialDoc, targetLine, diagnostics, onChange, onSave, handleRef }: RustEditorProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<EditorView | null>(null);
  // Latest callbacks behind refs so the CM extensions stay stable across renders.
  const onChangeRef = useRef(onChange);
  const onSaveRef = useRef(onSave);
  onChangeRef.current = onChange;
  onSaveRef.current = onSave;

  useImperativeHandle(
    handleRef,
    () => ({
      replaceDoc(text: string) {
        const view = viewRef.current;
        if (view === null) {
          return;
        }
        view.dispatch({
          changes: { from: 0, to: view.state.doc.length, insert: text },
          annotations: External.of(true),
        });
      },
      currentDoc: () => viewRef.current?.state.doc.toString() ?? '',
    }),
    [],
  );

  // Build the view once (the component is remounted per file via its key).
  useEffect(() => {
    const host = hostRef.current;
    if (host === null) {
      return;
    }
    const saveKeymap = keymap.of([
      {
        key: 'Mod-s',
        preventDefault: true,
        run: () => {
          onSaveRef.current();
          return true;
        },
      },
    ]);
    const changeListener = EditorView.updateListener.of((u) => {
      if (u.docChanged && !u.transactions.some((tr) => tr.annotation(External))) {
        onChangeRef.current(u.state.doc.toString());
      }
    });
    const view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: initialDoc,
        extensions: [
          lineNumbers(),
          highlightActiveLineGutter(),
          highlightActiveLine(),
          history(),
          indentOnInput(),
          bracketMatching(),
          highlightSelectionMatches(),
          search({ top: true }),
          rust(),
          syntaxHighlighting(meridianHighlight),
          targetLineField,
          lintGutter(),
          meridianTheme,
          saveKeymap,
          keymap.of([...searchKeymap, ...historyKeymap, indentWithTab, ...defaultKeymap]),
          changeListener,
        ],
      }),
    });
    viewRef.current = view;
    return () => {
      view.destroy();
      viewRef.current = null;
    };
    // `initialDoc` is intentionally read once at mount — a new file gets a new React key (fresh mount).
  }, []);

  // Target line: highlight + scroll into view whenever it changes.
  useEffect(() => {
    const view = viewRef.current;
    if (view === null) {
      return;
    }
    const effects: StateEffect<unknown>[] = [setTargetLine.of(targetLine)];
    if (targetLine >= 1 && targetLine <= view.state.doc.lines) {
      effects.push(EditorView.scrollIntoView(view.state.doc.line(targetLine).from, { y: 'center' }));
    }
    view.dispatch({ effects });
  }, [targetLine]);

  // Dev diagnostics → lint gutter + underlines.
  useEffect(() => {
    const view = viewRef.current;
    if (view === null) {
      return;
    }
    view.dispatch(setDiagnostics(view.state, toCMDiagnostics(view, diagnostics)));
  }, [diagnostics]);

  return <div className="cm-host" ref={hostRef} />;
}
