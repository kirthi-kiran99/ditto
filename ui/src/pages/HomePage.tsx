import { useState } from "react";
import { RecordingsTable } from "../components/RecordingsTable";
import { ReplayPanel } from "../components/ReplayPanel";

export function HomePage() {
  const [selectedId, setSelectedId] = useState<string | null>(null);

  function handleSelect(recordId: string) {
    setSelectedId((prev) => (prev === recordId ? null : recordId));
  }

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
      </header>

      {/* Split panel */}
      <div className="flex flex-1 overflow-hidden">
        {/* LEFT: compact recordings sidebar */}
        <aside className="w-80 border-r border-gray-200 bg-white flex flex-col overflow-hidden shrink-0">
          <div className="px-4 py-3 border-b border-gray-100">
            <h2 className="text-xs font-semibold text-gray-500 uppercase tracking-wide">Recordings</h2>
          </div>
          <RecordingsTable onSelect={handleSelect} selectedId={selectedId} />
        </aside>

        {/* RIGHT: replay detail pane */}
        <main className="flex-1 overflow-hidden">
          {selectedId ? (
            <ReplayPanel recordId={selectedId} />
          ) : (
            <EmptyState />
          )}
        </main>
      </div>
    </div>
  );
}

function EmptyState() {
  return (
    <div className="flex flex-col items-center justify-center h-full text-gray-400 gap-3">
      <svg className="w-12 h-12 opacity-30" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.2}>
        <path strokeLinecap="round" strokeLinejoin="round"
          d="M5.25 5.653c0-.856.917-1.398 1.667-.986l11.54 6.348a1.125 1.125 0 010 1.971l-11.54 6.347a1.125 1.125 0 01-1.667-.985V5.653z" />
      </svg>
      <p className="text-sm">Select a recording to replay</p>
    </div>
  );
}
