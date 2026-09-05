import { useId, type SVGProps, type CSSProperties } from "react";
import { TANGLEAF_PATHS } from "./geometry";
type Props = Omit<SVGProps<SVGSVGElement>, "children"> & { title?: string; branded?: boolean };
export function TangleafMark({ title, branded = false, width = 36, height = 36, ...props }: Props) {
  const id = useId();
  const color = (name: string, fallback: string) =>
    branded ? fallback : `var(--brand-${name}, ${fallback})`;
  const glow = (opacity: number) =>
    ({
      stopOpacity: branded ? opacity : `calc(var(--brand-glow, .55) * ${opacity / 0.55})`,
    }) as CSSProperties;
  return (
    <svg
      {...props}
      xmlns="http://www.w3.org/2000/svg"
      viewBox="0 0 560 560"
      width={width}
      height={height}
      focusable="false"
      role={title ? "img" : undefined}
      aria-hidden={title ? undefined : true}
      aria-labelledby={title ? `${id}-title` : undefined}
    >
      {title && <title id={`${id}-title`}>{title}</title>}
      <defs>
        <linearGradient
          id={`${id}-base`}
          gradientUnits="userSpaceOnUse"
          x1="80"
          y1="520"
          x2="440"
          y2="70"
        >
          <stop stopColor={color("start", "#143b66")} />
          <stop offset=".42" stopColor={color("mid", "#185e9f")} />
          <stop offset=".78" stopColor={color("end", "#278fd5")} />
          <stop offset="1" stopColor={color("end", "#278fd5")} />
        </linearGradient>
        <radialGradient id={`${id}-light`} gradientUnits="userSpaceOnUse" cx="495" cy="90" r="390">
          <stop stopColor={color("light", "#70d2eb")} style={glow(0.55)} />
          <stop offset=".40" stopColor={color("light", "#70d2eb")} style={glow(0.275)} />
          <stop offset="1" stopColor={color("light", "#70d2eb")} stopOpacity="0" />
        </radialGradient>
        <radialGradient id={`${id}-shade`} gradientUnits="userSpaceOnUse" cx="115" cy="500" r="340">
          <stop stopColor={color("start", "#143b66")} stopOpacity=".25" />
          <stop offset="1" stopColor={color("start", "#143b66")} stopOpacity="0" />
        </radialGradient>
      </defs>
      {(["base", "light", "shade"] as const).map((layer) => (
        <path key={layer} d={TANGLEAF_PATHS[0]} fill={`url(#${id}-${layer})`} />
      ))}
      <path d={TANGLEAF_PATHS[1]} fill="#fff" />
      <path d={TANGLEAF_PATHS[2]} fill="#fff" />
    </svg>
  );
}
