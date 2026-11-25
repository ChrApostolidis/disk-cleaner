interface Props {
  on: boolean;
  disabled: boolean;
  onChange: (on: boolean) => void;
}

/**
 * The "Fast scan" switch: a lightning bolt, the label, the Windows UAC shield (the
 * system's own sign for "this asks for administrator permission") and a sliding switch.
 */
export function FastScanSwitch({ on, disabled, onChange }: Props) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      className="fast-switch"
      disabled={disabled}
      onClick={() => onChange(!on)}
      title="Reads the drive's file table directly: much faster on big drives and sees every folder. Windows asks for administrator permission each time. NTFS drives only."
    >
      <svg className="fast-bolt" viewBox="0 0 16 16" aria-hidden="true">
        <path d="M9.5 1 3 9h4.2L6.5 15 13 7H8.8z" />
      </svg>
      <span>Fast scan</span>
      <svg className="fast-shield" viewBox="0 0 16 16" aria-label="Needs administrator permission" role="img">
        <path d="M8 1 2.5 3v4.2c0 3.6 2.3 6.3 5.5 7.8 3.2-1.5 5.5-4.2 5.5-7.8V3z" />
      </svg>
      <span className="fast-track" aria-hidden="true">
        <span className="fast-knob" />
      </span>
    </button>
  );
}
