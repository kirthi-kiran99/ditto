import { useState } from "react";
import { useQuery, useMutation } from "@tanstack/react-query";
import { api } from "../api";
import type { TagSummary, RunAllResult } from "../types";
import { RecordingsTable } from "../components/RecordingsTable";
import { ReplayPanel }     from "../components/ReplayPanel";
import { TagsList }        from "../components/TagsList";
import { RunAllPanel }     from "../components/RunAllPanel";

type View = "tags" | "recordings";
type RightPane = "empty" | "replay" | "run-all";

export function HomePage() {
  const [view, setView]               = useState<View>("tags");
  const [selectedTag, setSelectedTag] = useState<TagSummary | null>(null);
  const [selectedId,  setSelectedId]  = useState<string | null>(null);
  const [rightPane,   setRightPane]   = useState<RightPane>("empty");
  const [runAllResult, setRunAllResult] = useState<RunAllResult | null>(null);

  // ── Tag landing ─────────────────────────────────────────────────────────────
  const { data: tagsData, isLoading: tagsLoading } = useQuery({
    queryKey: ["tags"],
    queryFn:  () => api.listTags(),
    staleTime: 15_000,
    enabled:   view === "tags",
  });

  // ── Run-all mutation ─────────────────────────────────────────────────────────
  const runAllMutation = useMutation({
    mutationFn: () => api.replayAll(selectedTag!.service_name, selectedTag!.tag),
    onSuccess:  (data) => {
      setRunAllResult(data);
      setRightPane("run-all");
    },
  });

  // ── Handlers ─────────────────────────────────────────────────────────────────
  function handleSelectTag(tag: TagSummary) {
    setSelectedTag(tag);
    setSelectedId(null);
    setRightPane("empty");
    setRunAllResult(null);
    setView("recordings");
  }

  function handleBack() {
    setView("tags");
    setSelectedTag(null);
    setSelectedId(null);
    setRightPane("empty");
    setRunAllResult(null);
  }

  function handleSelectRecording(recordId: string) {
    setSelectedId((prev) => (prev === recordId ? null : recordId));
    setRightPane(recordId ? "replay" : "empty");
    setRunAllResult(null);
  }

  function handleSelectFromRunAll(recordId: string) {
    setSelectedId(recordId);
    setRightPane("replay");
  }

  // ── Render ───────────────────────────────────────────────────────────────────
  return (
    <div className="h-screen bg-gray-50 flex flex-col overflow-hidden">
      {/* Top nav */}
      <header className="bg-white border-b border-gray-200 px-6 py-3 flex items-center gap-3 shrink-0">
        <div className="w-7 h-7 rounded bg-indigo-600 flex items-center justify-center">
          <span className="text-white text-xs font-bold">d</span>
        </div>
        <div>
          <h1 className="text-base font-semibold text-gray-900 leading-tight">ditto</h1>
          <p className="text-xs text-gray-400 leading-tight">replay console</p>
        </div>

        {/* Breadcrumb */}
        {view === "recordings" && selectedTag && (
          <div className="ml-4 flex items-center gap-2 text-sm text-gray-500">
            <button
              onClick={handleBack}
              className="hover:text-indigo-600 transition-colors"
            >
              Tags
            </button>
            <span className="text-gray-300">/</span>
            <span className="text-gray-400 text-xs">{selectedTag.service_name}</span>
            <span className="text-gray-300">/</span>
            <span className="font-medium text-gray-700">{selectedTag.tag}</span>
          </div>
        )}

        {/* Run All button */}
        {view === "recordings" && selectedTag && (
          <button
            onClick={() => runAllMutation.mutate()}
            disabled={runAllMutation.isPending}
            className="ml-auto px-3 py-1.5 rounded bg-indigo-600 text-white text-xs font-medium
                       hover:bg-indigo-700 disabled:opacity-50 transition-colors"
          >
            {runAllMutation.isPending ? "Running…" : "Run All"}
          </button>
        )}
      </header>

      {/* ── Tags landing ─────────────────────────────────────────────────── */}
      {view === "tags" && (
        <div className="flex-1 overflow-y-auto">
          {tagsLoading ? (
            <div className="flex items-center justify-center h-full text-gray-400 text-sm gap-2">
              <svg className="animate-spin h-5 w-5" fill="none" viewBox="0 0 24 24">
                <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8v8H4z" />
              </svg>
              Loading tags…
            </div>
          ) : (
            <TagsList tags={tagsData?.items ?? []} onSelect={handleSelectTag} />
          )}
        </div>
      )}

      {/* ── Recordings split-panel ──────────────────────────────────────── */}
      {view === "recordings" && (
        <div className="flex flex-1 overflow-hidden">
          {/* LEFT: compact recordings sidebar */}
          <aside className="w-80 border-r border-gray-200 bg-white flex flex-col overflow-hidden shrink-0">
            <div className="px-4 py-3 border-b border-gray-100 shrink-0">
              <h2 className="text-xs font-semibold text-gray-500 uppercase tracking-wide">
                Recordings
              </h2>
            </div>
            <RecordingsTable
              onSelect={handleSelectRecording}
              selectedId={selectedId}
              filter={selectedTag ? { service_name: selectedTag.service_name, tag: selectedTag.tag } : undefined}
            />
          </aside>

          {/* RIGHT: replay / run-all pane */}
          <main className="flex-1 overflow-hidden">
            {rightPane === "replay" && selectedId ? (
              <ReplayPanel recordId={selectedId} />
            ) : rightPane === "run-all" && runAllResult ? (
              <RunAllPanel result={runAllResult} onSelectRecord={handleSelectFromRunAll} />
            ) : (
              <EmptyState hasTag={!!selectedTag} />
            )}
          </main>
        </div>
      )}
    </div>
  );
}

function EmptyState({ hasTag }: { hasTag: boolean }) {
  return (
    <div className="flex flex-col items-center justify-center h-full text-gray-400 gap-3">
      <svg className="w-12 h-12 opacity-30" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.2}>
        <path strokeLinecap="round" strokeLinejoin="round"
          d="M5.25 5.653c0-.856.917-1.398 1.667-.986l11.54 6.348a1.125 1.125 0 010 1.971l-11.54 6.347a1.125 1.125 0 01-1.667-.985V5.653z" />
      </svg>
      <p className="text-sm">
        {hasTag ? "Select a recording to replay, or click Run All" : "Select a recording to replay"}
      </p>
    </div>
  );
}
