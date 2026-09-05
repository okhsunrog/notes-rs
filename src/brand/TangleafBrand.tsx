import { TangleafMark } from "./TangleafMark";
import "./brand.css";
import "./typography.css";
export function TangleafBrand({
  branded = false,
  className = "",
}: {
  branded?: boolean;
  className?: string;
}) {
  return (
    <span className={`tangleaf-brand ${className}`}>
      <TangleafMark width={32} height={32} branded={branded} />
      <span className="tangleaf-wordmark">tangleaf</span>
    </span>
  );
}
