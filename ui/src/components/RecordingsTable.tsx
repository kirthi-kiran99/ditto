import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { api } from "../api";
import type { RecordingSummary } from "../types";

interface Props {
  onSelect:   (recordId: string) => void;
  selectedId: string | null;
  filter?:    { service_name: string; tag: string };
}

// ── Method badge ──────────────────────────────────────────────────────────────

const METHOD_STYLE: Record<string, string> = {
  GET:    "bg-blue-50   text-blue-700  border-blue-200",
  POST:   "bg-green-50  text-green-700 border-green-200",
  PUT:    "bg-amber-50  text-amber-700 border-amber-200",
  PATCH:  "bg-orange-50 text-orange-700 border-orange-200",
  DELETE: "bg-red-50    text-red-700   border-red-200",
};

function MethodBadge({ method }: { method: string }) {
  const m   = method.toUpperCase();
  const cls = METHOD_STYLE[m] ?? "bg-gray-50 text-gray-600 border-gray-200";
  return (
    <span className={`inline-block px-1.5 py-0.5 rounded border text-[10px] font-bold font-mono tracking-wide shrink-0 ${cls}`}>
      {m}
    </span>
  );
}

// ── Status dot ────────────────────────────────────────────────────────────────

function StatusDot({ code }: { code: number }) {
  const ok  = code >= 200 && code < 300;
  const dot = ok ? "bg-green-400" : "bg-red-400";
  return (
    <span className="inline-flex items-center gap-1">
      <span className={`inline-block w-1.5 h-1.5 rounded-full ${dot}`} />
      <span className="font-mono text-[10px] text-gray-500 tabular-nums">{code}</span>
    </span>
  );
}

// ── Date formatting ───────────────────────────────────────────────────────────

function formatDate(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleString(undefined, {
    month: "short", day: "numeric",
    hour: "2-digit", minute: "2-digit",
  });
}

// ── Component ─────────────────────────────────────────────────────────────────

const PAGE_SIZE = 20;

export function RecordingsTable({ onSelect, selectedId, filter }: Props) {
  const [offset, setOffset] = useState(0);

  const { data, isLoading, isError, error } = useQuery({
    queryKey: filter
      ? ["recordings", "tag", filter.service_name, filter.tag, offset]
      : ["recordings", offset],
    queryFn: filter
      ? () => api.listRecordingsByTag(filter.service_name, filter.tag, PAGE_SIZE, offset)
      : () => api.listRecordings(PAGE_SIZE, offset),
    staleTime: 10_000,
  });

  if (isLoading) {
    return (
      <div className="flex items-center justify-center py-10 text-gray-400 text-xs gap-2">
        <svg className="animate-spin h-4 w-4" fill="none" viewBox="0 0 24 24">
          <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
          <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8v8H4z" />
        </svg>
        Loading…
      </div>
    );
  }

  if (isError) {
    return (
      <div className="m-3 rounded-md bg-red-50 border border-red-200 px-3 py-2 text-xs text-red-700">
        {(error as Error).message}
      </div>
    );
  }

  if (!data || data.items.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center py-12 px-4 text-gray-400 gap-2 text-center">
        <svg className="w-7 h-7 opacity-40" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
          <path strokeLinecap="round" strokeLinejoin="round"
            d="M9 12h3.75M9 15h3.75M9 18h3.75m3 .75H18a2.25 2.25 0 002.25-2.25V6.108c0-1.135-.845-2.098-1.976-2.192a48.424 48.424 0 00-1.123-.08m-5.801 0c-.065.21-.1.433-.1.664 0 .414.336.75.75.75h4.5a.75.75 0 00.75-.75 2.25 2.25 0 00-.1-.664m-5.8 0A2.251 2.251 0 0113.5 2.25H15c1.012 0 1.867.668 2.15 1.586m-5.8 0c-.376.023-.75.05-1.124.08C9.095 4.01 8.25 4.973 8.25 6.108V8.25m0 0H4.875c-.621 0-1.125.504-1.125 1.125v11.25c0 .621.504 1.125 1.125 1.125h9.75c.621 0 1.125-.504 1.125-1.125V9.375c0-.621-.504-1.125-1.125-1.125H8.25zM6.75 12h.008v.008H6.75V12zm0 3h.008v.008H6.75V15zm0 3h.008v.008H6.75V18z" />
        </svg>
        <p className="text-xs">No recordings yet</p>
        <p className="text-[11px]">
          Run with{" "}
          <code className="px-1 py-0.5 bg-gray-100 rounded font-mono">REPLAY_MODE=record</code>
        </p>
      </div>
    );
  }

  const totalPages  = Math.ceil(data.total / PAGE_SIZE);
  const currentPage = Math.floor(offset / PAGE_SIZE) + 1;

  return (
    <div className="flex flex-col flex-1 overflow-hidden">
      {/* Scrollable list */}
      <div className="flex-1 overflow-y-auto divide-y divide-gray-100">
        {data.items.map((rec: RecordingSummary) => {
          const isSelected = rec.record_id === selectedId;
          return (
            <button
              key={rec.record_id}
              onClick={() => onSelect(rec.record_id)}
              className={`w-full text-left px-4 py-3 flex gap-2 transition-colors relative
                ${isSelected ? "bg-indigo-50" : "hover:bg-gray-50"}`}
            >
              {/* Selected accent bar */}
              <span className={`absolute left-0 top-2 bottom-2 w-0.5 rounded-full transition-colors
                ${isSelected ? "bg-indigo-500" : "bg-transparent"}`}
              />

              <div className="flex-1 min-w-0 space-y-1">
                {/* Top row: method + path */}
                <div className="flex items-center gap-2 min-w-0">
                  <MethodBadge method={rec.method} />
                  <span className={`text-xs font-mono truncate ${isSelected ? "text-indigo-900" : "text-gray-800"}`}>
                    {rec.path}
                  </span>
                </div>

                {/* Bottom row: status · calls · time */}
                <div className="flex items-center gap-2">
                  <StatusDot code={rec.status_code} />
                  <span className="text-gray-300 text-[10px]">·</span>
                  <span className="text-[10px] text-gray-400 tabular-nums">
                    {rec.interaction_count} call{rec.interaction_count !== 1 ? "s" : ""}
                  </span>
                  <span className="ml-auto text-[10px] text-gray-400 whitespace-nowrap tabular-nums">
                    {formatDate(rec.recorded_at)}
                  </span>
                </div>
              </div>
            </button>
          );
        })}
      </div>

      {/* Pagination */}
      {totalPages > 1 && (
        <div className="flex items-center justify-between px-4 py-2 border-t border-gray-100 text-[11px] text-gray-500 shrink-0">
          <span className="tabular-nums">
            {offset + 1}–{Math.min(offset + PAGE_SIZE, data.total)} of {data.total}
          </span>
          <div className="flex items-center gap-1">
            <PageButton
              onClick={() => setOffset(Math.max(0, offset - PAGE_SIZE))}
              disabled={offset === 0}
            >
              ←
            </PageButton>
            <span className="px-1.5 tabular-nums text-gray-400">{currentPage}/{totalPages}</span>
            <PageButton
              onClick={() => setOffset(offset + PAGE_SIZE)}
              disabled={offset + PAGE_SIZE >= data.total}
            >
              →
            </PageButton>
          </div>
        </div>
      )}
    </div>
  );
}

function PageButton({ onClick, disabled, children }: {
  onClick: () => void;
  disabled: boolean;
  children: React.ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      className="px-2 py-1 rounded border border-gray-200 text-xs disabled:opacity-40 hover:bg-gray-50 disabled:cursor-not-allowed transition-colors"
    >
      {children}
    </button>
  );
}
