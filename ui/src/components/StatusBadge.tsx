import type { ReplayStatus } from "../types";

interface Props {
  status: ReplayStatus | "recording";
  size?: "sm" | "lg";
}

const CONFIG: Record<string, { label: string; cls: string }> = {
  passed:    { label: "PASSED",    cls: "bg-green-100 text-green-800 border-green-300" },
  failed:    { label: "FAILED",    cls: "bg-red-100   text-red-800   border-red-300"   },
  error:     { label: "ERROR",     cls: "bg-yellow-100 text-yellow-800 border-yellow-300" },
  recording: { label: "RECORDED",  cls: "bg-blue-100  text-blue-800  border-blue-300"  },
};

export function StatusBadge({ status, size = "sm" }: Props) {
  const cfg = CONFIG[status] ?? CONFIG["error"];
  const sizeClass = size === "lg"
    ? "px-3 py-1 text-sm font-semibold rounded-md border"
    : "px-2 py-0.5 text-xs font-medium rounded border";
  return (
    <span className={`inline-flex items-center ${sizeClass} ${cfg.cls}`}>
      {cfg.label}
    </span>
  );
}
