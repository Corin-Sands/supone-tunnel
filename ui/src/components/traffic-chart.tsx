import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { runtimeTraffic, type TrafficSample } from "../api";

/** 最近 1 小时窗口；2s 采样约 1800 点，按时间戳裁剪。Author: Corin-Sands */
const WINDOW_MS = 60 * 60 * 1000;
const INTERVAL_MS = 2_000;
/** 圈选过窄则忽略，避免误触。Author: Corin-Sands */
const MIN_ZOOM_MS = 30_000;
const WIDTH = 640;
const HEIGHT = 168;
const PAD_L = 54;
const PAD_R = 14;
const PAD_T = 10;
const PAD_B = 26;

type Point = { t: number; up: number; down: number };
type ZoomRange = { start: number; end: number };

type TrafficSnapshot = {
  points: Point[];
  latest: TrafficSample;
  now: number;
  zoom: ZoomRange | null;
};

/** 模块级采样仓：切 tab 卸载组件也不丢点、不停采。Author: Corin-Sands */
const trafficStore: {
  snap: TrafficSnapshot;
  listeners: Set<() => void>;
  busy: boolean;
  timer: number | null;
  desiredRunning: boolean;
} = {
  snap: {
    points: [],
    latest: { up: 0, down: 0, running: false },
    now: Date.now(),
    zoom: null,
  },
  listeners: new Set(),
  busy: false,
  timer: null,
  desiredRunning: false,
};

function emitTraffic() {
  for (const listener of trafficStore.listeners) {
    listener();
  }
}

function getTrafficSnapshot(): TrafficSnapshot {
  return trafficStore.snap;
}

function subscribeTraffic(listener: () => void): () => void {
  trafficStore.listeners.add(listener);
  return () => {
    trafficStore.listeners.delete(listener);
  };
}

function setTrafficSnap(patch: Partial<TrafficSnapshot>) {
  trafficStore.snap = { ...trafficStore.snap, ...patch };
  emitTraffic();
}

function pushTrafficSample(sample: TrafficSample) {
  const t = Date.now();
  const cutoff = t - WINDOW_MS;
  const points = trafficStore.snap.points.filter((item) => item.t >= cutoff);
  points.push({ t, up: sample.up, down: sample.down });
  trafficStore.snap = {
    ...trafficStore.snap,
    points,
    latest: sample,
    now: t,
  };
  emitTraffic();
}

function clearTrafficHistory() {
  trafficStore.snap = {
    points: [],
    latest: { up: 0, down: 0, running: false },
    now: Date.now(),
    zoom: null,
  };
  emitTraffic();
}

async function trafficTick() {
  if (trafficStore.busy || !trafficStore.desiredRunning) {
    return;
  }
  trafficStore.busy = true;
  try {
    const sample = await runtimeTraffic();
    if (!trafficStore.desiredRunning) {
      return;
    }
    pushTrafficSample(sample);
  } catch {
    // 采样失败时保持上一帧，避免图表抖动。
  } finally {
    trafficStore.busy = false;
  }
}

function stopTrafficTimer() {
  if (trafficStore.timer != null) {
    window.clearInterval(trafficStore.timer);
    trafficStore.timer = null;
  }
}

/** 切走页面不停止采样；仅内核停止时清历史。Author: Corin-Sands */
export function syncTrafficSampling(running: boolean) {
  setTrafficDesiredRunning(running);
}

function setTrafficDesiredRunning(running: boolean) {
  if (trafficStore.desiredRunning === running) {
    if (running && trafficStore.timer == null) {
      void trafficTick();
      trafficStore.timer = window.setInterval(() => void trafficTick(), INTERVAL_MS);
    }
    return;
  }
  trafficStore.desiredRunning = running;
  if (!running) {
    stopTrafficTimer();
    clearTrafficHistory();
    return;
  }
  stopTrafficTimer();
  void trafficTick();
  trafficStore.timer = window.setInterval(() => void trafficTick(), INTERVAL_MS);
}

function setTrafficZoom(zoom: ZoomRange | null) {
  setTrafficSnap({ zoom });
}

function formatRate(bytesPerSec: number): string {
  if (bytesPerSec < 1024) {
    return `${Math.round(bytesPerSec)} B/s`;
  }
  if (bytesPerSec < 1024 * 1024) {
    const kb = bytesPerSec / 1024;
    return `${kb >= 10 ? kb.toFixed(0) : kb.toFixed(1)} KB/s`;
  }
  const mb = bytesPerSec / (1024 * 1024);
  return `${mb >= 10 ? mb.toFixed(1) : mb.toFixed(2)} MB/s`;
}

/** 纵轴刻度取整到 1/2/5×10ⁿ，避免挤成一排怪数字。Author: Corin-Sands */
function niceMax(raw: number): number {
  if (raw <= 0) {
    return 1024;
  }
  const exp = Math.floor(Math.log10(raw));
  const base = 10 ** exp;
  const n = raw / base;
  if (n <= 1) {
    return base;
  }
  if (n <= 2) {
    return 2 * base;
  }
  if (n <= 5) {
    return 5 * base;
  }
  return 10 * base;
}

function formatClock(ts: number): string {
  const d = new Date(ts);
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  const ss = String(d.getSeconds()).padStart(2, "0");
  if (d.getSeconds() === 0) {
    return `${hh}:${mm}`;
  }
  return `${hh}:${mm}:${ss}`;
}

function plotX(t: number, windowStart: number, windowSpan: number): number {
  const innerW = WIDTH - PAD_L - PAD_R;
  const span = Math.max(1, windowSpan);
  const ratio = Math.min(1, Math.max(0, (t - windowStart) / span));
  return PAD_L + ratio * innerW;
}

function plotY(value: number, max: number): number {
  const innerH = HEIGHT - PAD_T - PAD_B;
  return PAD_T + innerH - (max <= 0 ? 0 : (value / max) * innerH);
}

function buildPath(
  points: Point[],
  key: "up" | "down",
  windowStart: number,
  windowSpan: number,
  max: number,
): string {
  if (points.length === 0) {
    return "";
  }
  return points
    .map((point, index) => {
      const x = plotX(point.t, windowStart, windowSpan);
      const y = plotY(point[key], max);
      return `${index === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
}

function buildArea(
  points: Point[],
  key: "up" | "down",
  windowStart: number,
  windowSpan: number,
  max: number,
): string {
  const line = buildPath(points, key, windowStart, windowSpan, max);
  if (!line || points.length === 0) {
    return "";
  }
  const baseY = HEIGHT - PAD_B;
  const endX = plotX(points[points.length - 1].t, windowStart, windowSpan);
  const startX = plotX(points[0].t, windowStart, windowSpan);
  return `${line} L${endX.toFixed(1)},${baseY} L${startX.toFixed(1)},${baseY} Z`;
}

function clientToSvgX(svg: SVGSVGElement, clientX: number): number {
  const point = svg.createSVGPoint();
  point.x = clientX;
  point.y = 0;
  const ctm = svg.getScreenCTM();
  if (!ctm) {
    return PAD_L;
  }
  return point.matrixTransform(ctm.inverse()).x;
}

function svgXToTime(x: number, windowStart: number, windowSpan: number): number {
  const innerW = WIDTH - PAD_L - PAD_R;
  const clamped = Math.min(WIDTH - PAD_R, Math.max(PAD_L, x));
  const ratio = (clamped - PAD_L) / innerW;
  return windowStart + ratio * windowSpan;
}

/** 状态页流量图：持久采样 + 圈选缩放 + 回归默认。Author: Corin-Sands */
export function TrafficChart(props: { running: boolean }) {
  const { running } = props;
  const snap = useSyncExternalStore(subscribeTraffic, getTrafficSnapshot, getTrafficSnapshot);
  const svgRef = useRef<SVGSVGElement | null>(null);
  const dragRef = useRef<{ originX: number; currentX: number } | null>(null);
  const [brush, setBrush] = useState<{ x1: number; x2: number } | null>(null);

  useEffect(() => {
    setTrafficDesiredRunning(running);
  }, [running]);

  const chart = useMemo(() => {
    const liveStart = snap.now - WINDOW_MS;
    const liveEnd = snap.now;
    const windowStart = snap.zoom?.start ?? liveStart;
    const windowEnd = snap.zoom?.end ?? liveEnd;
    const windowSpan = Math.max(1, windowEnd - windowStart);
    const visible = snap.points.filter((item) => item.t >= windowStart && item.t <= windowEnd);
    const peak = Math.max(0, ...visible.map((item) => Math.max(item.up, item.down)));
    const max = niceMax(peak);
    const yTicks = [0, 0.5, 1].map((ratio) => ({
      value: max * ratio,
      y: plotY(max * ratio, max),
      label: formatRate(max * ratio),
    }));
    const xTicks = [0, 0.25, 0.5, 0.75, 1].map((ratio) => {
      const t = windowStart + ratio * windowSpan;
      return {
        t,
        x: plotX(t, windowStart, windowSpan),
        label: formatClock(t),
      };
    });
    return {
      windowStart,
      windowEnd,
      windowSpan,
      max,
      yTicks,
      xTicks,
      upLine: buildPath(visible, "up", windowStart, windowSpan, max),
      downLine: buildPath(visible, "down", windowStart, windowSpan, max),
      upArea: buildArea(visible, "up", windowStart, windowSpan, max),
      downArea: buildArea(visible, "down", windowStart, windowSpan, max),
      hasData: visible.length > 0,
      zoomed: snap.zoom != null,
    };
  }, [snap]);

  const finishBrush = useCallback(
    (originX: number, currentX: number) => {
      const left = Math.min(originX, currentX);
      const right = Math.max(originX, currentX);
      if (right - left < 8) {
        setBrush(null);
        return;
      }
      const start = svgXToTime(left, chart.windowStart, chart.windowSpan);
      const end = svgXToTime(right, chart.windowStart, chart.windowSpan);
      if (end - start < MIN_ZOOM_MS) {
        setBrush(null);
        return;
      }
      setTrafficZoom({ start, end });
      setBrush(null);
    },
    [chart.windowStart, chart.windowSpan],
  );

  const onPointerDown = (event: ReactPointerEvent<SVGSVGElement>) => {
    if (!running || event.button !== 0) {
      return;
    }
    const svg = svgRef.current;
    if (!svg) {
      return;
    }
    const x = clientToSvgX(svg, event.clientX);
    if (x < PAD_L || x > WIDTH - PAD_R) {
      return;
    }
    svg.setPointerCapture(event.pointerId);
    dragRef.current = { originX: x, currentX: x };
    setBrush({ x1: x, x2: x });
  };

  const onPointerMove = (event: ReactPointerEvent<SVGSVGElement>) => {
    if (!dragRef.current || !svgRef.current) {
      return;
    }
    const x = Math.min(WIDTH - PAD_R, Math.max(PAD_L, clientToSvgX(svgRef.current, event.clientX)));
    dragRef.current.currentX = x;
    setBrush({ x1: dragRef.current.originX, x2: x });
  };

  const onPointerUp = (event: ReactPointerEvent<SVGSVGElement>) => {
    if (!dragRef.current) {
      return;
    }
    const { originX, currentX } = dragRef.current;
    dragRef.current = null;
    try {
      svgRef.current?.releasePointerCapture(event.pointerId);
    } catch {
      // ignore
    }
    finishBrush(originX, currentX);
  };

  const resetZoom = () => {
    setTrafficZoom(null);
    setBrush(null);
    dragRef.current = null;
  };

  const mode = running ? "running" : "stopped";
  const baseY = HEIGHT - PAD_B;
  const plotRight = WIDTH - PAD_R;
  const brushLeft = brush ? Math.min(brush.x1, brush.x2) : 0;
  const brushWidth = brush ? Math.abs(brush.x2 - brush.x1) : 0;

  return (
    <div className={`traffic-chart traffic-chart--${mode}${chart.zoomed ? " traffic-chart--zoomed" : ""}`}>
      <div className="traffic-chart__head">
        <strong>流量</strong>
        <span className={`traffic-chart__badge traffic-chart__badge--${mode}`}>
          {!running ? "已停止" : chart.zoomed ? "圈选区间" : "最近 1 小时"}
        </span>
        {running && chart.zoomed && (
          <button type="button" className="traffic-chart__reset" onClick={resetZoom}>
            回归默认
          </button>
        )}
        <span className="traffic-chart__rates">
          <span className="traffic-chart__up">↑ {formatRate(snap.latest.up)}</span>
          <span className="traffic-chart__down">↓ {formatRate(snap.latest.down)}</span>
        </span>
      </div>
      <svg
        ref={svgRef}
        className={`traffic-chart__svg${running ? " traffic-chart__svg--selectable" : ""}`}
        viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
        preserveAspectRatio="xMidYMid meet"
        role="img"
        aria-label={
          running
            ? chart.zoomed
              ? "圈选后的上下行流量，可回归默认"
              : "最近一小时上下行流量，可拖选放大"
            : "内核已停止，流量为零"
        }
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
        onDoubleClick={resetZoom}
      >
        {chart.yTicks.map((tick) => (
          <g key={`y-${tick.value}`}>
            <line
              className="traffic-chart__grid"
              x1={PAD_L}
              y1={tick.y}
              x2={plotRight}
              y2={tick.y}
            />
            <text className="traffic-chart__axis-label traffic-chart__axis-label--y" x={PAD_L - 6} y={tick.y + 3}>
              {tick.label}
            </text>
          </g>
        ))}
        {chart.xTicks.map((tick) => (
          <g key={`x-${tick.t}`}>
            <line
              className="traffic-chart__tick"
              x1={tick.x}
              y1={baseY}
              x2={tick.x}
              y2={baseY + 4}
            />
            <text className="traffic-chart__axis-label traffic-chart__axis-label--x" x={tick.x} y={HEIGHT - 6}>
              {tick.label}
            </text>
          </g>
        ))}
        <line className="traffic-chart__axis" x1={PAD_L} y1={PAD_T} x2={PAD_L} y2={baseY} />
        <line className="traffic-chart__axis" x1={PAD_L} y1={baseY} x2={plotRight} y2={baseY} />
        {running && chart.hasData ? (
          <>
            <path className="traffic-chart__area traffic-chart__area--down" d={chart.downArea} />
            <path className="traffic-chart__area traffic-chart__area--up" d={chart.upArea} />
            <path className="traffic-chart__line traffic-chart__line--down" d={chart.downLine} />
            <path className="traffic-chart__line traffic-chart__line--up" d={chart.upLine} />
          </>
        ) : (
          <path
            className="traffic-chart__line traffic-chart__line--idle"
            d={`M${PAD_L},${baseY} L${plotRight},${baseY}`}
          />
        )}
        {brush && brushWidth > 0 && (
          <rect
            className="traffic-chart__brush"
            x={brushLeft}
            y={PAD_T}
            width={brushWidth}
            height={baseY - PAD_T}
          />
        )}
      </svg>
      {running ? (
        <p className="traffic-chart__hint">
          {chart.zoomed ? "双击图表或点「回归默认」回到最近 1 小时" : "在图上拖选一段时间可放大查看"}
        </p>
      ) : (
        <p className="traffic-chart__hint">启动内核后开始采样上下行</p>
      )}
    </div>
  );
}
