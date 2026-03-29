import type { RunAllResult } from "../types";

interface Props {
  result:         RunAllResult;
  onSelectRecord: (id: string) => void;
}

function StatusBadge({ status }: { status: string }) {
  const cls =
    status === "passed"
      ? "bg-green-100 text-green-700"
      : status === "failed"
      ? "bg-red-100 text-red-700"
      : "bg-yellow-100 text-yellow-700";
  return (
    <span className={`inline-block px-2 py-0.5 rounded text-xs font-medium ${cls}`}>
      {status}
    </span>
  );
}

export function RunAllPanel({ result, onSelectRecord }: Props) {
  const { total, passed, failed, errors, duration_ms, results } = result;

  return (
    <div className="h-full flex flex-col bg-gray-50">
      {/* Summary bar */}
      <div className="shrink-0 px-6 py-4 border-b border-gray-200 bg-white">
        <h2 className="text-sm font-semibold text-gray-700 mb-2">
          Run All — {result.service_name} / {result.tag}
        </h2>
        <div className="flex flex-wrap gap-4 text-sm">
          <span className="text-gray-500">
            Total: <span className="font-medium text-gray-900">{total}</span>
          </span>
          <span className="text-green-600">
            Passed: <span className="font-medium">{passed}</span>
          </span>
          <span className="text-red-600">
            Failed: <span className="font-medium">{failed}</span>
          </span>
          {errors > 0 && (
            <span className="text-yellow-600">
              Errors: <span className="font-medium">{errors}</span>
            </span>
          )}
          <span className="text-gray-400">{duration_ms}ms total</span>
        </div>
      </div>

      {/* Per-recording rows */}
      <div className="flex-1 overflow-y-auto divide-y divide-gray-100">
        {results.map((r) => (
          <button
            key={r.record_id}
            onClick={() => onSelectRecord(r.record_id)}
            className="w-full flex items-center gap-3 px-6 py-3 text-left hover:bg-white transition-colors"
          >
            <StatusBadge status={r.status} />
            <span className="flex-1 truncate font-mono text-xs text-gray-600">
              {r.record_id}
            </span>
            {r.mismatches.length > 0 && (
              <span className="text-xs text-red-500 shrink-0">
                {r.mismatches.length} mismatch{r.mismatches.length > 1 ? "es" : ""}
              </span>
            )}
            {r.error && (
              <span className="text-xs text-yellow-600 shrink-0 truncate max-w-[180px]" title={r.error}>
                {r.error}
              </span>
            )}
            <span className="text-xs text-gray-400 shrink-0">{r.duration_ms}ms</span>
            <span className="text-gray-300 text-sm">›</span>
          </button>
        ))}
      </div>
    </div>
  );
}
