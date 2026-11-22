/**
 * The Ballast symbol: a porthole with the water held low and two bubbles rising.
 * The ring follows the text color and the water uses the theme's --accent, so it
 * matches light and dark mode. Source: logos/final/symbol.svg.
 */
export function BrandMark({ size = 28 }: { size?: number }) {
  return (
    <svg className="brand-mark" viewBox="0 0 256 256" width={size} height={size} aria-hidden="true">
      <path
        fill="currentColor"
        fillRule="evenodd"
        d="M16 128A112 112 0 1 1 240 128A112 112 0 1 1 16 128Z M32 128A96 96 0 1 0 224 128A96 96 0 1 0 32 128Z"
      />
      <path fill="var(--accent)" d="M41.47 144H214.53A88 88 0 0 1 41.47 144Z" />
      <circle fill="var(--accent)" cx="148" cy="124" r="12" />
      <circle fill="var(--accent)" cx="176" cy="84" r="20" />
    </svg>
  );
}
