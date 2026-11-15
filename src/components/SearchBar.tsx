interface Props {
  value: string;
  onChange: (v: string) => void;
  error: string | null;
}

const PRESETS: { label: string; token: string }[] = [
  { label: "Older than 90 days", token: "older:90d" },
  { label: "Bigger than 500 MB", token: ">500MB" },
];

function tokens(q: string): string[] {
  return q.match(/"[^"]*"|\S+/g) ?? [];
}

function toggleToken(q: string, token: string): string {
  const parts = tokens(q);
  const next = parts.includes(token) ? parts.filter((t) => t !== token) : [...parts, token];
  return next.join(" ");
}

export function SearchBar({ value, onChange, error }: Props) {
  const active = new Set(tokens(value));
  return (
    <div className="searchbar">
      <div className="search-row">
        <input
          type="search"
          className="search-input"
          placeholder='Search: words, *.mp3, "two words", >500MB, older:90d'
          value={value}
          onChange={(e) => onChange(e.target.value)}
          aria-invalid={error !== null}
          aria-label="Search"
          spellCheck={false}
        />
        <div className="chips">
          {PRESETS.map((p) => (
            <button
              key={p.token}
              className={`chip${active.has(p.token) ? " chip-on" : ""}`}
              aria-pressed={active.has(p.token)}
              onClick={() => onChange(toggleToken(value, p.token))}
            >
              {p.label}
            </button>
          ))}
        </div>
      </div>
      {error && (
        <div className="search-error" role="alert">
          {error}
        </div>
      )}
    </div>
  );
}
