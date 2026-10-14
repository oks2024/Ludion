// The timeline surface. Canvas, not SVG: the stream is 60 columns a second over a three-minute ring,
// so an SVG node per column would churn thousands of elements per window for marks that are one to
// two pixels wide. Nothing here decides anything — `model.ts`'s `layoutTimeline` hands it pixels and
// this file fills rectangles.
//
// Classes: `pf-plot` (the positioned box) and `pf-canvas`; every colour comes from a token read off
// the element, never a literal.

import { useCallback, useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react';
import {
  columnAt,
  layoutTimeline,
  readColumn,
  type ColumnReadout,
  type Series,
  type SeriesSet,
} from './model';
import { PALETTE_SLOTS } from './colors';

export interface TimelineCanvasProps {
  readonly set: SeriesSet;
  /** The lane's bands, bottom of the stack first — already ranked and folded. */
  readonly series: readonly Series[];
  /** The palette slot a band wears, or `null` for the folded remainder. */
  readonly slotOf: (key: string) => number | null;
  /** Whether to draw the frame-total line over the stack (the sim lane only — a GPU stack is not a
   *  share of the tick, so a tick line over it would invite a comparison that means nothing). */
  readonly showTotal: boolean;
  readonly onHover: (readout: ColumnReadout | null) => void;
}

/** The tokens the canvas needs as concrete colours; `fillStyle` cannot resolve a `var()`. */
interface Palette {
  readonly slots: readonly string[];
  readonly other: string;
  readonly guide: string;
  readonly total: string;
  readonly axis: string;
  readonly hover: string;
}

function readPalette(element: HTMLElement): Palette {
  const style = getComputedStyle(element);
  const token = (name: string, fallback: string): string => style.getPropertyValue(name).trim() || fallback;
  return {
    slots: Array.from({ length: PALETTE_SLOTS }, (_, i) => token(`--pf-cat-${i}`, '#888')),
    other: token('--pf-other', '#5c6270'),
    guide: token('--text-3', '#5c6270'),
    total: token('--text-2', '#9ba0ab'),
    axis: token('--border', '#23262c'),
    hover: token('--text', '#edeef2'),
  };
}

export function TimelineCanvas({ set, series, slotOf, showTotal, onHover }: TimelineCanvasProps) {
  const boxRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  const [palette, setPalette] = useState<Palette | null>(null);
  const [hoverColumn, setHoverColumn] = useState<number | null>(null);

  // The box is a flex child of an unknown width, so its size is observed rather than passed in.
  useEffect(() => {
    const box = boxRef.current;
    if (box === null) {
      return;
    }
    setPalette(readPalette(box));
    const observer = new ResizeObserver((entries) => {
      const rect = entries[0]?.contentRect;
      if (rect !== undefined) {
        setSize({ width: Math.floor(rect.width), height: Math.floor(rect.height) });
      }
    });
    observer.observe(box);
    return () => observer.disconnect();
  }, []);

  const layout = useMemo(
    () => layoutTimeline(set, series, { width: size.width, height: size.height }),
    [set, series, size.width, size.height],
  );

  useEffect(() => {
    const canvas = canvasRef.current;
    if (canvas === null || palette === null || size.width <= 0 || size.height <= 0) {
      return;
    }
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(size.width * dpr);
    canvas.height = Math.round(size.height * dpr);
    const ctx = canvas.getContext('2d');
    if (ctx === null) {
      return;
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, size.width, size.height);

    // Baseline, so an empty plot still reads as an axis rather than as a broken panel.
    ctx.fillStyle = palette.axis;
    ctx.fillRect(0, size.height - 1, size.width, 1);

    const width = Math.max(layout.columnWidth, 1);
    layout.bands.forEach((band) => {
      const slot = slotOf(band.key);
      ctx.fillStyle = slot === null ? palette.other : (palette.slots[slot % PALETTE_SLOTS] ?? palette.other);
      for (let c = 0; c < band.top.length; c += 1) {
        const height = band.bottom[c] - band.top[c];
        if (height > 0) {
          ctx.fillRect(layout.x[c], band.top[c], width, height);
        }
      }
    });

    if (showTotal) {
      ctx.strokeStyle = palette.total;
      ctx.lineWidth = 1;
      ctx.beginPath();
      let open = false;
      layout.totalY.forEach((y, c) => {
        if (y === null) {
          open = false;
          return;
        }
        const x = layout.x[c] + width / 2;
        if (open) {
          ctx.lineTo(x, y);
        } else {
          ctx.moveTo(x, y);
          open = true;
        }
      });
      ctx.stroke();
    }

    if (layout.guideY !== null) {
      ctx.save();
      ctx.strokeStyle = palette.guide;
      ctx.lineWidth = 1;
      ctx.setLineDash([3, 3]);
      ctx.beginPath();
      ctx.moveTo(0, layout.guideY + 0.5);
      ctx.lineTo(size.width, layout.guideY + 0.5);
      ctx.stroke();
      ctx.restore();
    }

    if (hoverColumn !== null && hoverColumn < layout.x.length) {
      ctx.fillStyle = palette.hover;
      ctx.globalAlpha = 0.18;
      ctx.fillRect(layout.x[hoverColumn], 0, width, size.height);
      ctx.globalAlpha = 1;
    }
  }, [layout, palette, size.width, size.height, series, slotOf, showTotal, hoverColumn]);

  const handleMove = useCallback(
    (event: ReactPointerEvent<HTMLCanvasElement>): void => {
      const rect = event.currentTarget.getBoundingClientRect();
      const column = columnAt(layout, event.clientX - rect.left);
      setHoverColumn(column);
      onHover(column === null ? null : readColumn(set, series, layout, column));
    },
    [layout, set, series, onHover],
  );

  const handleLeave = useCallback((): void => {
    setHoverColumn(null);
    onHover(null);
  }, [onHover]);

  return (
    <div className="pf-plot" ref={boxRef}>
      <canvas
        className="pf-canvas"
        ref={canvasRef}
        style={{ width: size.width, height: size.height }}
        onPointerMove={handleMove}
        onPointerLeave={handleLeave}
      />
      <span className="pf-axis-max num">{layout.maxMs} ms</span>
      <span className="pf-axis-guide num" style={layout.guideY === null ? undefined : { top: layout.guideY }}>
        16.6 ms
      </span>
    </div>
  );
}
