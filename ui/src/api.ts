import type {
  RecordingDetail,
  RecordingsPage,
  ReplayResult,
  RunAllResult,
  TagsPage,
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

  listTags(limit = 50, offset = 0): Promise<TagsPage> {
    return request<TagsPage>(`/tags?limit=${limit}&offset=${offset}`);
  },

  listRecordingsByTag(
    serviceName: string,
    tag: string,
    limit = 20,
    offset = 0
  ): Promise<RecordingsPage> {
    const q = new URLSearchParams({
      service_name: serviceName,
      tag,
      limit:  String(limit),
      offset: String(offset),
    });
    return request<RecordingsPage>(`/tags/recordings?${q}`);
  },

  replayAll(serviceName: string, tag: string): Promise<RunAllResult> {
    const q = new URLSearchParams({ service_name: serviceName, tag });
    return request<RunAllResult>(`/tags/replay-all?${q}`, { method: "POST" });
  },
};
