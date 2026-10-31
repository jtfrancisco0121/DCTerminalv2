import type { TrendPoint } from "../run/analytics";

/** Inline SVG charts (no chart library). Colours come from `.chart-*` in App.css. */

const W = 600;
const H = 200;
const LEFT = 52;
const RIGHT = 12;
const TOP = 10;
const BOTTOM = 22;
const PLOT_W = W - LEFT - RIGHT;
const PLOT_H = H - TOP - BOTTOM;

type Props = {
  /** What the chart shows, for the heading and the accessible name. */
  title: string;
  points: ReadonlyArray<TrendPoint>;
  /** Words a value ("$1.50", "35 min", "25%"). */
  format: (value: number) => string;
};

const day = (at: string) => at.slice(0, 10);

/** The accessible name: what, how many runs, the date range and the value range. */
function describe({ title, points, format }: Props): string {
  const values = points.map((p) => p.value);
  return `${title}: ${points.length} runs from ${day(points[0].at)} to ${day(points[points.length - 1].at)}, values from ${format(Math.min(...values))} to ${format(Math.max(...values))}`;
}

function Empty({ title }: { title: string }) {
  return <p className="hint chart-empty">{title}: at least two finished runs are needed to draw a chart.</p>;
}

function Axes({ top, format }: { top: number; format: (value: number) => string }) {
  return (
    <g className="chart-axis" aria-hidden>
      <line x1={LEFT} y1={TOP} x2={LEFT} y2={TOP + PLOT_H} />
      <line x1={LEFT} y1={TOP + PLOT_H} x2={LEFT + PLOT_W} y2={TOP + PLOT_H} />
      <text x={LEFT - 6} y={TOP + 4} textAnchor="end">
        {format(top)}
      </text>
      <text x={LEFT - 6} y={TOP + PLOT_H} textAnchor="end">
        {format(0)}
      </text>
    </g>
  );
}

const markTitle = (p: TrendPoint, format: (value: number) => string) => `${p.label} · ${day(p.at)} · ${format(p.value)}`;

/** One bar per run, oldest first, from zero. */
export function BarChart(props: Props) {
  const { points, format, title } = props;
  if (points.length < 2) return <Empty title={title} />;
  const top = Math.max(...points.map((p) => p.value), Number.MIN_VALUE);
  const slot = PLOT_W / points.length;
  const width = Math.max(2, Math.min(40, slot * 0.7));
  return (
    <svg className="chart" viewBox={`0 0 ${W} ${H}`} role="img" aria-label={describe(props)}>
      <Axes top={top} format={format} />
      {points.map((p, at) => {
        const height = (p.value / top) * PLOT_H;
        return (
          <rect
            key={p.runId}
            className="chart-bar"
            tabIndex={0}
            x={LEFT + slot * at + (slot - width) / 2}
            y={TOP + PLOT_H - height}
            width={width}
            height={height}
          >
            <title>{markTitle(p, format)}</title>
          </rect>
        );
      })}
    </svg>
  );
}

/** A line over the runs, oldest first, on a fixed 0 to 1 scale (shares). */
export function LineChart(props: Props) {
  const { points, format, title } = props;
  if (points.length < 2) return <Empty title={title} />;
  const x = (at: number) => LEFT + (PLOT_W * at) / (points.length - 1);
  const y = (value: number) => TOP + PLOT_H - Math.min(1, Math.max(0, value)) * PLOT_H;
  return (
    <svg className="chart" viewBox={`0 0 ${W} ${H}`} role="img" aria-label={describe(props)}>
      <Axes top={1} format={format} />
      <polyline className="chart-line" points={points.map((p, at) => `${x(at)},${y(p.value)}`).join(" ")} />
      {points.map((p, at) => (
        <circle key={p.runId} className="chart-dot" tabIndex={0} cx={x(at)} cy={y(p.value)} r={4}>
          <title>{markTitle(p, format)}</title>
        </circle>
      ))}
    </svg>
  );
}
