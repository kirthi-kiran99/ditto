import type { MismatchDetail } from "../types";

interface Props {
  mismatch: MismatchDetail;
}

const CATEGORY_STYLE: Record<string, string> = {
  regression:  "bg-red-50   border-l-4 border-red-400",
  added:       "bg-green-50 border-l-4 border-green-400",
  removed:     "bg-red-50   border-l-4 border-red-400",
  intentional: "bg-amber-50 border-l-4 border-amber-400",
  noise:       "bg-gray-50  opacity-50",
};

const CATEGORY_CHIP: Record<string, string> = {
  regression:  "bg-red-100   text-red-700",
  added:       "bg-green-100 text-green-700",
  removed:     "bg-red-100   text-red-700",
  intentional: "bg-amber-100 text-amber-700",
  noise:       "bg-gray-100  text-gray-500",
};

function pretty(v: unknown): string {
  if (v === null || v === undefined) return "—";
  if (typeof v === "string") return v;
  return JSON.stringify(v, null, 2);
}

export function DiffViewer({ mismatch }: Props) {
  if (mismatch.nodes.length === 0) {
    return (
      <p className="text-sm text-gray-500 italic px-4 py-2">
        No field-level diffs for this interaction.
      </p>
    );
  }

  return (
    <div className="overflow-x-auto rounded border border-gray-200 mt-2">
      <table className="min-w-full text-xs font-mono">
        <thead>
          <tr className="bg-gray-100 text-gray-600 text-left">
            <th className="px-3 py-2 w-1/4">Field path</th>
            <th className="px-3 py-2 w-1/3">Recorded value</th>
            <th className="px-3 py-2 w-1/3">Replayed value</th>
            <th className="px-3 py-2">Category</th>
          </tr>
        </thead>
        <tbody className="divide-y divide-gray-200">
          {mismatch.nodes.map((node, i) => (
            <tr key={i} className={CATEGORY_STYLE[node.category] ?? ""}>
              <td className="px-3 py-2 align-top text-gray-700 font-semibold break-all">
                {node.path}
              </td>
              <td className="px-3 py-2 align-top text-red-700 whitespace-pre-wrap break-all">
                {pretty(node.old_value)}
              </td>
              <td className="px-3 py-2 align-top text-green-700 whitespace-pre-wrap break-all">
                {pretty(node.new_value)}
              </td>
              <td className="px-3 py-2 align-top">
                <span
                  className={`px-2 py-0.5 rounded text-xs font-medium ${
                    CATEGORY_CHIP[node.category] ?? "bg-gray-100 text-gray-500"
                  }`}
                >
                  {node.category}
                </span>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
