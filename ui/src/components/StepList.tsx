import { useState } from "react";
import type { InteractionDetail, ReplayResult } from "../types";
import { DiffViewer } from "./DiffViewer";
import { RawViewer } from "./RawViewer";

// ── Call-type badge ───────────────────────────────────────────────────────────

const CALL_TYPE_STYLE: Record<string, string> = {
  http:     "bg-blue-100  text-blue-700",
  grpc:     "bg-violet-100 text-violet-700",
  postgres: "bg-sky-100   text-sky-700",
  redis:    "bg-red-100   text-red-700",
  function: "bg-amber-100 text-amber-700",
};

const CALL_TYPE_LABEL: Record<string, string> = {
  http:     "HTTP",
  grpc:     "gRPC",
  postgres: "PG",
  redis:    "Redis",
  function: "fn()",
};

function CallTypeBadge({ type }: { type: string }) {
  const style = CALL_TYPE_STYLE[type] ?? "bg-gray-100 text-gray-600";
  const label = CALL_TYPE_LABEL[type] ?? type;
  return (
    <span className={`inline-block px-1.5 py-0.5 rounded text-[10px] font-semibold tracking-wide shrink-0 ${style}`}>
      {label}
    </span>
  );
}

// ── Row status config ─────────────────────────────────────────────────────────

type RowStatus = "idle" | "matched" | "regression" | "diff" | "missing";

const LEFT_BORDER: Record<RowStatus, string> = {
  idle:       "border-l-2 border-transparent",
  matched:    "border-l-2 border-green-400",
  regression: "border-l-2 border-red-500",
  diff:       "border-l-2 border-amber-400",
  missing:    "border-l-2 border-red-300",
};

const ROW_BG: Record<RowStatus, string> = {
  idle:       "hover:bg-gray-50",
  matched:    "hover:bg-green-50",
  regression: "bg-red-50 hover:bg-red-100",
  diff:       "bg-amber-50 hover:bg-amber-100",
  missing:    "bg-red-50 hover:bg-red-50",
};

// ── Types ─────────────────────────────────────────────────────────────────────

type PanelTab = "diff" | "raw";

interface Props {
  interactions: InteractionDetail[];
  replayResult: ReplayResult | null;
}

// ── Component ─────────────────────────────────────────────────────────────────

export function StepList({ interactions, replayResult }: Props) {
  const [expanded, setExpanded]   = useState<number | null>(null);
  const [activeTab, setActiveTab] = useState<Record<number, PanelTab>>({});

  const mismatchBySeq = new Map(
    (replayResult?.mismatches ?? []).map((m) => [m.sequence, m])
  );
  const missingSet = new Set(replayResult?.missing_replays ?? []);

  function getStatus(seq: number): RowStatus {
    if (!replayResult) return "idle";
    if (missingSet.has(seq)) return "missing";
    const m = mismatchBySeq.get(seq);
    if (m?.is_regression) return "regression";
    if (m) return "diff";
    return "matched";
  }

  function getTab(seq: number): PanelTab {
    return activeTab[seq] ?? (mismatchBySeq.has(seq) ? "diff" : "raw");
  }

  function setTab(seq: number, tab: PanelTab) {
    setActiveTab((prev) => ({ ...prev, [seq]: tab }));
  }

  return (
    <div className="divide-y divide-gray-100">
      {interactions.map((interaction) => {
        const seq       = interaction.sequence;
        const status    = getStatus(seq);
        const mismatch  = mismatchBySeq.get(seq);
        const isExpanded = expanded === seq;
        const tab       = getTab(seq);
        const hasDiff   = !!mismatch && mismatch.nodes.length > 0;

        // ── Status badge ────────────────────────────────────────────────────
        const badge = (() => {
          if (!replayResult) return null;
          if (status === "missing")
            return <span className="inline-flex items-center gap-1 text-xs text-red-600 font-medium"><Dot className="bg-red-400" />missing</span>;
          if (status === "regression")
            return <span className="inline-flex items-center gap-1 text-xs text-red-600 font-medium"><Dot className="bg-red-500" />{mismatch!.nodes.length} regression{mismatch!.nodes.length !== 1 ? "s" : ""}</span>;
          if (status === "diff")
            return <span className="inline-flex items-center gap-1 text-xs text-amber-600 font-medium"><Dot className="bg-amber-400" />{mismatch!.nodes.length} diff{mismatch!.nodes.length !== 1 ? "s" : ""}</span>;
          return <span className="inline-flex items-center gap-1 text-xs text-green-600 font-medium"><Dot className="bg-green-400" />matched</span>;
        })();

        return (
          <div key={seq}>
            {/* ── Row ─────────────────────────────────────────────────────── */}
            <div
              className={`flex items-center gap-3 px-4 py-3 cursor-pointer select-none transition-colors
                ${LEFT_BORDER[status]} ${ROW_BG[status]}`}
              onClick={() => setExpanded(isExpanded ? null : seq)}
            >
              {/* Sequence */}
              <span className="text-[11px] text-gray-400 w-5 text-right shrink-0 tabular-nums">
                {seq}
              </span>

              {/* Call type */}
              <CallTypeBadge type={interaction.call_type} />

              {/* Fingerprint */}
              <span className="flex-1 text-sm font-mono text-gray-800 truncate">
                {interaction.fingerprint}
              </span>

              {/* Duration */}
              <span className="text-xs text-gray-400 shrink-0 tabular-nums">
                {interaction.duration_ms}ms
              </span>

              {/* Status badge */}
              {badge && <span className="shrink-0">{badge}</span>}

              {/* Chevron */}
              <svg
                className={`w-3.5 h-3.5 shrink-0 text-gray-400 transition-transform duration-150 ${isExpanded ? "rotate-180" : ""}`}
                fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}
              >
                <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
              </svg>
            </div>

            {/* ── Expanded panel ───────────────────────────────────────────── */}
            {isExpanded && status !== "missing" && (
              <div className="bg-white border-t border-gray-100 px-4 pb-4 pt-3">
                {/* Tab bar — only show tabs when both views are available */}
                {hasDiff && (
                  <div className="flex gap-1 mb-3">
                    <TabButton active={tab === "diff"} onClick={() => setTab(seq, "diff")}>
                      Diff
                      {status === "regression" && (
                        <span className="ml-1.5 px-1 py-0.5 rounded bg-red-100 text-red-600 text-[10px] font-semibold">
                          {mismatch!.nodes.length}
                        </span>
                      )}
                      {status === "diff" && (
                        <span className="ml-1.5 px-1 py-0.5 rounded bg-amber-100 text-amber-600 text-[10px] font-semibold">
                          {mismatch!.nodes.length}
                        </span>
                      )}
                    </TabButton>
                    <TabButton active={tab === "raw"} onClick={() => setTab(seq, "raw")}>
                      Raw
                    </TabButton>
                  </div>
                )}

                {/* Content */}
                {hasDiff && tab === "diff" ? (
                  <DiffViewer mismatch={mismatch!} />
                ) : (
                  <RawViewer interaction={interaction} />
                )}
              </div>
            )}

            {/* ── Missing: show a message instead ─────────────────────────── */}
            {isExpanded && status === "missing" && (
              <div className="bg-red-50 border-t border-red-100 px-4 py-3 text-sm text-red-600 italic">
                This interaction was not replayed — the service did not make this call during replay.
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

// ── Small helpers ─────────────────────────────────────────────────────────────

function Dot({ className }: { className: string }) {
  return <span className={`inline-block w-1.5 h-1.5 rounded-full ${className}`} />;
}

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      className={`inline-flex items-center px-3 py-1 rounded text-xs font-medium transition-colors
        ${active
          ? "bg-gray-900 text-white"
          : "bg-gray-100 text-gray-600 hover:bg-gray-200"
        }`}
    >
      {children}
    </button>
  );
}
