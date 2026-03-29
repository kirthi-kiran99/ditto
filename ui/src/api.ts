import type {
  RecordingDetail,
  RecordingsPage,
  ReplayResult,
} from "./types";

const BASE = "/api";

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`${BASE}${path}`, init);
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText);
    throw new Error(`${res.status} ${res.statusText}: ${text}`);
  }
  return res.json() as Promise<T>;
}

export const api = {
  listRecordings(limit = 20, offset = 0): Promise<RecordingsPage> {
    return request<RecordingsPage>(
      `/recordings?limit=${limit}&offset=${offset}`
    );
  },

  getRecording(id: string): Promise<RecordingDetail> {
    return request<RecordingDetail>(`/recordings/${id}`);
  },

  triggerReplay(id: string): Promise<ReplayResult> {
    return request<ReplayResult>(`/recordings/${id}/replay`, {
      method: "POST",
    });
  },
};
