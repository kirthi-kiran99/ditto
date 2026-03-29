import type { TagSummary } from "../types";

interface Props {
  tags:     TagSummary[];
  onSelect: (tag: TagSummary) => void;
}

function formatDate(iso: string): string {
  return new Date(iso).toLocaleString(undefined, {
    month: "short",
    day:   "numeric",
    hour:  "2-digit",
    minute: "2-digit",
  });
}

export function TagsList({ tags, onSelect }: Props) {
  if (tags.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center h-full text-gray-400 gap-2">
        <p className="text-lg font-medium">No recordings yet</p>
        <p className="text-sm">
          Run your service with{" "}
          <code className="bg-gray-100 px-1 rounded text-xs">REPLAY_MODE=record REPLAY_TAG=my-scenario</code>
        </p>
      </div>
    );
  }

  return (
    <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4 p-6">
      {tags.map((t) => (
        <button
          key={`${t.service_name}::${t.tag}`}
          onClick={() => onSelect(t)}
          className="text-left bg-white border border-gray-200 rounded-xl p-5 shadow-sm
                     hover:border-indigo-400 hover:shadow-md transition-all duration-150 group"
        >
          <div className="flex items-start justify-between gap-2 mb-3">
            <div className="min-w-0">
              <p className="text-xs font-medium text-gray-400 uppercase tracking-wide truncate">
                {t.service_name || "unknown service"}
              </p>
              <p className="text-base font-semibold text-gray-900 mt-0.5 truncate group-hover:text-indigo-700">
                {t.tag}
              </p>
            </div>
            <span className="shrink-0 text-indigo-500 opacity-0 group-hover:opacity-100 transition-opacity text-lg">
              →
            </span>
          </div>

          <div className="flex gap-4 text-sm text-gray-600">
            <span>
              <span className="font-medium text-gray-900">{t.recording_count}</span>{" "}
              {t.recording_count === 1 ? "recording" : "recordings"}
            </span>
            <span>
              <span className="font-medium text-gray-900">{t.interaction_count}</span>{" "}
              interactions
            </span>
          </div>

          <p className="mt-3 text-xs text-gray-400">
            Last recorded {formatDate(t.last_recorded_at)}
          </p>
        </button>
      ))}
    </div>
  );
}
