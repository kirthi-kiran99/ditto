// ── Recordings list ─────────────────────────────────────────────────────────

export interface RecordingSummary {
  record_id:         string;
  method:            string;
  path:              string;
  status_code:       number;
  recorded_at:       string; // ISO-8601
  build_hash:        string;
  service_name:      string;
  interaction_count: number;
}

export interface RecordingsPage {
  items:  RecordingSummary[];
  total:  number;
  limit:  number;
  offset: number;
}

// ── Recording detail ────────────────────────────────────────────────────────

export type CallType   = "http" | "grpc" | "postgres" | "redis" | "function";
export type CallStatus = "completed" | "error" | "cancelled" | "timeout";

export interface InteractionDetail {
  id:          string;
  sequence:    number;
  call_type:   CallType;
  fingerprint: string;
  request:     Record<string, unknown>;
  response:    Record<string, unknown>;
  duration_ms: number;
  status:      CallStatus;
  error:       string | null;
}

export interface RecordingDetail {
  record_id:    string;
  interactions: InteractionDetail[];
}

// ── Replay result ────────────────────────────────────────────────────────────

export type DiffCategory =
  | "noise"
  | "regression"
  | "intentional"
  | "added"
  | "removed";

export type ReplayStatus = "passed" | "failed" | "error";

export interface DiffNode {
  path:      string;
  old_value: unknown;
  new_value: unknown;
  category:  DiffCategory;
}

export interface MismatchDetail {
  sequence:      number;
  fingerprint:   string;
  is_regression: boolean;
  nodes:         DiffNode[];
}

export interface ReplayResult {
  record_id:       string;
  status:          ReplayStatus;
  is_regression:   boolean;
  duration_ms:     number;
  matched:         number;
  missing_replays: number[];
  extra_replays:   number[];
  mismatches:      MismatchDetail[];
}
