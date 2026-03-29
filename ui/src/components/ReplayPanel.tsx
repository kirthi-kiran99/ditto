import { useMutation, useQuery } from "@tanstack/react-query";
import { api } from "../api";
import type { ReplayResult } from "../types";
import { StatusBadge } from "./StatusBadge";
import { StepList } from "./StepList";

interface Props {
  recordId: string;
}

export function ReplayPanel({ recordId }: Props) {
  const { data: detail, isLoading: detailLoading } = useQuery({
    queryKey: ["recording", recordId],
    queryFn:  () => api.getRecording(recordId),
  });

  const replay = useMutation<ReplayResult, Error>({
    mutationFn: () => api.triggerReplay(recordId),
  });

  const regressionCount = replay.data?.mismatches.filter((m) => m.is_regression).length ?? 0;
  const diffCount       = replay.data?.mismatches.filter((m) => !m.is_regression).length ?? 0;

  return (
    <div className="h-full flex flex-col bg-gray-50">
      {/* ── Header ─────────────────────────────────────────────────────────── */}
      <div className="flex items-center justify-between px-6 py-4 border-b border-gray-200 bg-white shrink-0">
        <div className="min-w-0">
          <p className="text-[11px] text-gray-400 font-mono truncate">{recordId}</p>
          <h3 className="text-sm font-semibold text-gray-900 mt-0.5">Interaction trace</h3>
        </div>

        <div className="flex items-center gap-3 ml-4 shrink-0">
          {replay.data && <StatusBadge status={replay.data.status} size="lg" />}

          <button
            onClick={() => replay.mutate()}
            disabled={replay.isPending}
            className={`inline-flex items-center gap-2 px-4 py-2 rounded-md text-sm font-medium
              transition-colors focus:outline-none focus:ring-2 focus:ring-offset-1
              ${replay.isPending
                ? "bg-gray-100 text-gray-400 cursor-not-allowed"
                : "bg-indigo-600 text-white hover:bg-indigo-700 focus:ring-indigo-500"
              }`}
          >
            {replay.isPending ? (
              <><Spinner />Running…</>
            ) : (
              <><PlayIcon />Replay</>
            )}
          </button>
        </div>
      </div>

      {/* ── Summary pills ──────────────────────────────────────────────────── */}
      {replay.data && (
        <div className="flex flex-wrap items-center gap-2 px-6 py-3 border-b border-gray-200 bg-white shrink-0">
          <Pill value={replay.data.matched}              label="matched"   color="green" />
          <Pill value={replay.data.missing_replays.length} label="missing" color={replay.data.missing_replays.length > 0 ? "red" : "gray"} />
          <Pill value={replay.data.extra_replays.length}   label="extra"   color={replay.data.extra_replays.length > 0 ? "amber" : "gray"} />
          <Pill value={diffCount}                          label="diffs"   color={diffCount > 0 ? "amber" : "gray"} />
          <Pill value={regressionCount}                    label={regressionCount === 1 ? "regression" : "regressions"} color={regressionCount > 0 ? "red" : "gray"} />
          <span className="ml-auto text-xs text-gray-400 tabular-nums">{replay.data.duration_ms} ms</span>
        </div>
      )}

      {/* ── Error ──────────────────────────────────────────────────────────── */}
      {replay.isError && (
        <div className="mx-6 mt-4 rounded-md bg-red-50 border border-red-200 px-4 py-3 text-sm text-red-700">
          {replay.error.message}
        </div>
      )}

      {/* ── Loading ────────────────────────────────────────────────────────── */}
      {detailLoading && (
        <div className="flex items-center justify-center py-10 text-gray-400 text-sm gap-2">
          <Spinner />Loading interactions…
        </div>
      )}

      {/* ── Step list ──────────────────────────────────────────────────────── */}
      {detail && (
        <div className="mx-6 my-4 rounded-lg border border-gray-200 overflow-y-auto bg-white shadow-sm flex-1">
          {/* Column headers */}
          <div className="grid grid-cols-[28px_52px_1fr_56px_100px_20px] gap-3 px-4 py-2 bg-gray-50 border-b border-gray-200 text-[11px] font-medium text-gray-400 uppercase tracking-wide">
            <span className="text-right">#</span>
            <span>Type</span>
            <span>Fingerprint</span>
            <span className="text-right">ms</span>
            <span>Status</span>
            <span />
          </div>
          <StepList
            interactions={detail.interactions}
            replayResult={replay.data ?? null}
          />
        </div>
      )}
    </div>
  );
}

// ── Pill stat ─────────────────────────────────────────────────────────────────

type PillColor = "green" | "red" | "amber" | "gray";

const PILL_STYLE: Record<PillColor, string> = {
  green: "bg-green-50  text-green-700  border-green-200",
  red:   "bg-red-50    text-red-700    border-red-200",
  amber: "bg-amber-50  text-amber-700  border-amber-200",
  gray:  "bg-gray-50   text-gray-500   border-gray-200",
};

function Pill({ value, label, color }: { value: number; label: string; color: PillColor }) {
  return (
    <span className={`inline-flex items-center gap-1 px-2.5 py-1 rounded-full border text-xs font-medium ${PILL_STYLE[color]}`}>
      <span className="font-semibold tabular-nums">{value}</span>
      <span className="font-normal">{label}</span>
    </span>
  );
}

// ── Icons ─────────────────────────────────────────────────────────────────────

function PlayIcon() {
  return (
    <svg className="w-3.5 h-3.5" fill="currentColor" viewBox="0 0 20 20">
      <path d="M6.3 2.84A1.5 1.5 0 004 4.11v11.78a1.5 1.5 0 002.3 1.27l9.344-5.891a1.5 1.5 0 000-2.538L6.3 2.84z" />
    </svg>
  );
}

function Spinner() {
  return (
    <svg className="animate-spin h-4 w-4" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
      <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
      <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8v8H4z" />
    </svg>
  );
}
