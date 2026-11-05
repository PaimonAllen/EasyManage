export interface MarkdownDocument {
  content: string;
  revision: number;
  updated_at: string | null;
  max_bytes: number;
}
export const GPU_DOCUMENT_URL = "/api/v1/documents/gpu-comparison";

async function readDocument(response: Response): Promise<MarkdownDocument> {
  if (!response.ok) {
    let message = `文档接口异常（HTTP ${response.status}）`;
    try { const error = await response.json() as { message?: string }; message = error.message ?? message; }
    catch { /* Preserve an informative status if a proxy returned HTML. */ }
    throw new Error(message);
  }
  return response.json() as Promise<MarkdownDocument>;
}
export async function fetchGpuDocument(signal?: AbortSignal): Promise<MarkdownDocument> {
  return readDocument(await fetch(GPU_DOCUMENT_URL, { cache: "no-store", signal }));
}
export async function saveGpuDocument(content: string, expectedRevision: number, source: "edit" | "upload", signal?: AbortSignal): Promise<MarkdownDocument> {
  return readDocument(await fetch(GPU_DOCUMENT_URL, { method: "PUT", cache: "no-store", signal,
    headers: { "Content-Type": "application/json" }, body: JSON.stringify({ content, expected_revision: expectedRevision, source }),
  }));
}
