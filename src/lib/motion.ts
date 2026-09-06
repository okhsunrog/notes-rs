/**
 * Scroll behavior for programmatic scrolling. `scroll-behavior: auto` in CSS only covers
 * scrolling the browser starts; a `behavior: "smooth"` passed to `scrollIntoView` overrides
 * it, so callers have to ask. On a panel that repaints per frame the animation is the cost,
 * not the jump.
 */
export function scrollBehavior(): ScrollBehavior {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return "smooth";
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth";
}
