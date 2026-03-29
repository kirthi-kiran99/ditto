import type { InteractionDetail } from "../types";

interface Props {
  interaction: InteractionDetail;
}

function JsonBlock({ label, value }: { label: string; value: unknown }) {
  const text =
    value === null || value === undefined
      ? "—"
      : JSON.stringify(value, null, 2);

  return (
    <div className="flex-1 min-w-0">
      <p className="text-xs font-semibold text-gray-500 uppercase tracking-wide mb-1.5">
        {label}
      </p>
      <pre className="text-xs font-mono text-gray-700 bg-gray-50 border border-gray-200 rounded p-3 overflow-auto max-h-64 leading-relaxed">
        {text}
      </pre>
    </div>
  );
}

export function RawViewer({ interaction }: Props) {
  return (
    <div className="mt-2 space-y-3">
      <div className="flex gap-3">
        <JsonBlock label="Recorded request"  value={interaction.request}  />
        <JsonBlock label="Recorded response" value={interaction.response} />
      </div>
      <div className="flex items-center gap-3 text-xs text-gray-400">
        <span className="font-mono">{interaction.call_type}</span>
        <span>·</span>
        <span>{interaction.duration_ms} ms</span>
        {interaction.error && (
          <>
            <span>·</span>
            <span className="text-red-500">{interaction.error}</span>
          </>
        )}
      </div>
    </div>
  );
}
